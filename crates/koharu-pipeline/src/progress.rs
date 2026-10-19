use std::sync::Arc;

use crate::Stage;
use koharu_scene::EntityId;

#[derive(Clone, Debug)]
pub enum Progress {
    Started {
        pages: Vec<EntityId>,
        stages: Vec<Stage>,
    },
    Loading {
        page: EntityId,
        stage: Stage,
        model: String,
    },
    Running {
        page: EntityId,
        stage: Stage,
        model: String,
    },
    Finished {
        page: EntityId,
        stage: Stage,
        model: String,
        elapsed: std::time::Duration,
    },
    Skipped {
        page: EntityId,
        stage: Stage,
    },
    /// The batch is pausing between pages; `next` starts after `remaining`.
    Waiting {
        next: EntityId,
        remaining: std::time::Duration,
    },
}

pub type ProgressSink = Arc<dyn Fn(Progress) + Send + Sync>;

pub(crate) fn emit(sink: Option<&ProgressSink>, progress: Progress) {
    if let Some(sink) = sink {
        sink(progress);
    }
}
