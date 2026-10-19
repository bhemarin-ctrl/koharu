use std::{
    collections::HashMap,
    fmt,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result};
use koharu_pipeline::{Committer, Progress, RunStatus, StageOutput, StopToken};
use koharu_scene::Snapshot;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager as _, State, ipc::Channel};
use tauri_runtime_cef::CefRuntime;
use uuid::Uuid;

use super::{ChannelExt as _, Error, canvas::CanvasChannel, project::CurrentProject};
use koharu_desktop::Desktop;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, Type)]
#[serde(transparent)]
pub struct JobId(Uuid);

impl JobId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct Job {
    pub id: JobId,
    pub state: JobState,
    #[specta(type = f64)]
    pub completed: usize,
    #[specta(type = f64)]
    pub total: usize,
    pub page: Option<koharu_scene::EntityId>,
    pub stage: Option<koharu_pipeline::Stage>,
    pub model: Option<String>,
    /// While a paced batch pauses between pages, when the next page starts,
    /// in milliseconds since the Unix epoch.
    pub resumes_at: Option<f64>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Finished,
    Failed,
    Stopped,
}

#[derive(Default)]
pub(crate) struct Processing {
    pub(crate) stops: Mutex<HashMap<JobId, StopToken>>,
    pub(crate) jobs: Mutex<HashMap<JobId, Job>>,
    pub(crate) inpainting_mask: Mutex<Option<koharu_pipeline::InpaintingMask>>,
}

#[derive(Default)]
pub(crate) struct JobChannel {
    pub(crate) channel: Mutex<Option<Channel<Job>>>,
}

#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn process(
    handle: AppHandle<CefRuntime>,
    scope: koharu_pipeline::Scope,
    operation: koharu_pipeline::Operation,
    page_interval_seconds: Option<u32>,
    project: State<'_, CurrentProject>,
    processing: State<'_, Processing>,
    job_channel: State<'_, JobChannel>,
) -> std::result::Result<JobId, Error> {
    let snapshot = project
        .project
        .lock()
        .await
        .as_ref()
        .context("no project is open")?
        .snapshot();
    let id = JobId::new();
    let stop = StopToken::default();
    {
        let mut stops = processing.stops.lock();
        if !stops.is_empty() {
            return Err(anyhow::anyhow!("another process is already running").into());
        }
        stops.insert(id, stop.clone());
    }
    let job = Job {
        id,
        state: JobState::Running,
        completed: 0,
        total: 0,
        page: None,
        stage: None,
        model: None,
        resumes_at: None,
        error: None,
    };
    processing.jobs.lock().insert(id, job.clone());
    job_channel.channel.publish(job);

    let pipeline = handle.state::<koharu_pipeline::Pipeline>().inner().clone();
    let task_handle = handle.clone();
    let inpainting_mask = processing.inpainting_mask.lock().take();
    drop(tokio::spawn(async move {
        let progress = Arc::new(Mutex::new((0_usize, 0_usize)));
        let progress_handle = task_handle.clone();
        let mut request = koharu_pipeline::Request {
            operation,
            scope,
            stop: stop.clone(),
            progress: None,
            inpainting_mask,
            page_interval: page_interval_seconds
                .filter(|seconds| *seconds > 0)
                .map(|seconds| Duration::from_secs(u64::from(seconds))),
        };
        request.progress = Some(Arc::new(move |event| {
            let update = match event {
                Progress::Started { pages, stages } => {
                    let mut progress = progress.lock();
                    *progress = (0, pages.len().saturating_mul(stages.len()));
                    Some(JobUpdate::step(0, progress.1, None, None, None))
                }
                Progress::Loading { page, stage, model } => {
                    let progress = progress.lock();
                    Some(JobUpdate::step(
                        progress.0,
                        progress.1,
                        Some(page),
                        Some(stage),
                        Some(model),
                    ))
                }
                Progress::Finished {
                    page, stage, model, ..
                } => {
                    let mut progress = progress.lock();
                    progress.0 = progress.0.saturating_add(1).min(progress.1);
                    Some(JobUpdate::step(
                        progress.0,
                        progress.1,
                        Some(page),
                        Some(stage),
                        Some(model),
                    ))
                }
                Progress::Skipped { page, stage } => {
                    let mut progress = progress.lock();
                    progress.0 = progress.0.saturating_add(1).min(progress.1);
                    Some(JobUpdate::step(
                        progress.0,
                        progress.1,
                        Some(page),
                        Some(stage),
                        None,
                    ))
                }
                Progress::Waiting { next, remaining } => {
                    let progress = progress.lock();
                    let resumes_at = (SystemTime::now() + remaining)
                        .duration_since(UNIX_EPOCH)
                        .map(|time| time.as_secs_f64() * 1000.0)
                        .ok();
                    Some(JobUpdate {
                        completed: progress.0,
                        total: progress.1,
                        page: Some(next),
                        stage: None,
                        model: None,
                        resumes_at,
                    })
                }
                Progress::Running { .. } => None,
            };
            if let Some(update) = update {
                let job = {
                    let processing = progress_handle.state::<Processing>();
                    let mut jobs = processing.jobs.lock();
                    jobs.get_mut(&id).map(|job| {
                        job.completed = update.completed;
                        job.total = update.total;
                        job.page = update.page;
                        job.stage = update.stage;
                        job.model = update.model;
                        job.resumes_at = update.resumes_at;
                        job.clone()
                    })
                };
                if let Some(job) = job {
                    progress_handle.state::<JobChannel>().channel.publish(job);
                }
            }
        }));

        struct PipelineCommitter {
            handle: AppHandle<CefRuntime>,
        }

        #[async_trait::async_trait]
        impl Committer for PipelineCommitter {
            async fn commit(&mut self, output: StageOutput) -> Result<Snapshot> {
                let (commit, page) = {
                    let projects = self.handle.state::<CurrentProject>();
                    let mut projects = projects.project.lock().await;
                    let project = projects.as_mut().context("no project is open")?;
                    let Some(commit) = project.commit_rebased(output.patch).await? else {
                        return Ok(project.snapshot());
                    };
                    project.record_commit(&commit);
                    let page = project.active_page();
                    (commit, page)
                };
                let snapshot = commit.snapshot.clone();
                let desktop = self.handle.state::<Desktop>();
                desktop.synchronize(&commit.snapshot, page, &commit).await?;
                let canvas = desktop.canvas_state();
                self.handle.state::<CanvasChannel>().channel.publish(canvas);
                Ok(snapshot)
            }
        }

        let mut committer = PipelineCommitter {
            handle: task_handle.clone(),
        };
        let result = pipeline.execute(snapshot, request, &mut committer).await;
        let (stopped, error) = match result {
            Ok(report) => (report.status == RunStatus::Stopped, None),
            Err(error) => {
                tracing::error!(stage = ?error.stage, %error, "processing failed");
                (false, Some(format!("{error:#}")))
            }
        };
        task_handle.state::<Processing>().stops.lock().remove(&id);
        let job = task_handle
            .state::<Processing>()
            .jobs
            .lock()
            .remove(&id)
            .map(|mut job| {
                job.state = if stopped {
                    JobState::Stopped
                } else if error.is_some() {
                    JobState::Failed
                } else {
                    JobState::Finished
                };
                job.error = error;
                job
            });
        if let Some(job) = job {
            task_handle.state::<JobChannel>().channel.publish(job);
        }
    }));
    Ok(id)
}

struct JobUpdate {
    completed: usize,
    total: usize,
    page: Option<koharu_scene::EntityId>,
    stage: Option<koharu_pipeline::Stage>,
    model: Option<String>,
    resumes_at: Option<f64>,
}

impl JobUpdate {
    fn step(
        completed: usize,
        total: usize,
        page: Option<koharu_scene::EntityId>,
        stage: Option<koharu_pipeline::Stage>,
        model: Option<String>,
    ) -> Self {
        Self {
            completed,
            total,
            page,
            stage,
            model,
            resumes_at: None,
        }
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn stop_job(
    job: JobId,
    processing: State<'_, Processing>,
) -> std::result::Result<(), Error> {
    let stops = processing.stops.lock();
    let stop = stops
        .get(&job)
        .with_context(|| format!("job {job} is not running"))?;
    stop.stop();
    Ok(())
}
