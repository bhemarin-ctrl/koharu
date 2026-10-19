use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

use koharu_scene::EntityId;

use crate::Stage;

#[derive(Clone, Copy, Eq, PartialEq)]
enum WorkState {
    Pending,
    Running,
    Finished,
}

struct StageWork {
    stage: Stage,
    state: WorkState,
}

struct PageWork {
    page: EntityId,
    stages: Vec<StageWork>,
}

impl PageWork {
    fn started(&self) -> bool {
        self.stages
            .iter()
            .any(|work| work.state != WorkState::Pending)
    }

    fn finished(&self) -> bool {
        self.stages
            .iter()
            .all(|work| work.state == WorkState::Finished)
    }

    fn ready(&self, index: usize) -> bool {
        prerequisites(self.stages[index].stage)
            .iter()
            .all(|prerequisite| self.finished_or_absent(*prerequisite))
    }

    fn finished_or_absent(&self, stage: Stage) -> bool {
        self.stages
            .iter()
            .find(|work| work.stage == stage)
            .is_none_or(|work| work.state == WorkState::Finished)
    }
}

pub(crate) struct Scheduler {
    pages: Vec<PageWork>,
    page_index: BTreeMap<EntityId, usize>,
    page_window: usize,
    active_pages: usize,
    head: usize,
    total: usize,
    page_interval: Option<Duration>,
    resume_at: Option<Instant>,
}

impl Scheduler {
    pub(crate) fn new(
        pages: &[EntityId],
        stages: &[Stage],
        page_interval: Option<Duration>,
    ) -> Self {
        let page_interval = page_interval.filter(|interval| !interval.is_zero());
        let pages = pages
            .iter()
            .map(|page| PageWork {
                page: *page,
                stages: stages
                    .iter()
                    .map(|stage| StageWork {
                        stage: *stage,
                        state: WorkState::Pending,
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        let total = pages.len().saturating_mul(stages.len());
        Self {
            page_index: pages
                .iter()
                .enumerate()
                .map(|(index, page)| (page.page, index))
                .collect(),
            pages,
            // A paced batch finishes one page completely before the next one
            // starts; otherwise every stage may work on its own page.
            page_window: if page_interval.is_some() {
                1
            } else {
                stages.len().max(1)
            },
            active_pages: 0,
            head: 0,
            total,
            page_interval,
            resume_at: None,
        }
    }

    /// When the batch is pausing between pages, the next page and the moment
    /// it may start.
    pub(crate) fn waiting(&self) -> Option<(EntityId, Instant)> {
        let resume_at = self.resume_at?;
        if Instant::now() >= resume_at {
            return None;
        }
        self.pages[self.head..]
            .iter()
            .find(|page| !page.started())
            .map(|page| (page.page, resume_at))
    }

    pub(crate) fn total(&self) -> usize {
        self.total
    }

    pub(crate) fn start_next(
        &mut self,
        busy_stages: &BTreeSet<Stage>,
    ) -> Option<(EntityId, Stage)> {
        for page_index in self.head..self.pages.len() {
            let started = self.pages[page_index].started();
            if !started
                && (self.active_pages >= self.page_window
                    || self
                        .resume_at
                        .is_some_and(|resume_at| Instant::now() < resume_at))
            {
                break;
            }
            let stage_index =
                self.pages[page_index]
                    .stages
                    .iter()
                    .enumerate()
                    .find_map(|(index, work)| {
                        (work.state == WorkState::Pending
                            && !busy_stages.contains(&work.stage)
                            && self.pages[page_index].ready(index)
                            && self.earlier_pages_finished(page_index, work.stage))
                        .then_some(index)
                    });
            let Some(stage_index) = stage_index else {
                continue;
            };
            if !started {
                self.active_pages += 1;
            }
            let page = &mut self.pages[page_index];
            let work = &mut page.stages[stage_index];
            work.state = WorkState::Running;
            return Some((page.page, work.stage));
        }
        None
    }

    pub(crate) fn complete_stage(&mut self, page: EntityId, stage: Stage) -> bool {
        let Some(&page_index) = self.page_index.get(&page) else {
            return false;
        };
        let page = &mut self.pages[page_index];
        let was_finished = page.finished();
        if let Some(work) = page.stages.iter_mut().find(|work| work.stage == stage) {
            work.state = WorkState::Finished;
        }
        let page_finished = !was_finished && page.finished();
        if page_finished {
            self.active_pages = self.active_pages.saturating_sub(1);
            while self.head < self.pages.len() && self.pages[self.head].finished() {
                self.head += 1;
            }
            if let Some(interval) = self.page_interval
                && self.head < self.pages.len()
            {
                self.resume_at = Some(Instant::now() + interval);
            }
        }
        page_finished
    }

    /// Scene summaries and translations read the pages before them, so a page
    /// runs those stages only after every earlier page in the batch has.
    fn earlier_pages_finished(&self, page_index: usize, stage: Stage) -> bool {
        if !matches!(stage, Stage::Summary | Stage::Translation) {
            return true;
        }
        self.pages[self.head..page_index]
            .iter()
            .all(|page| page.finished_or_absent(stage))
    }
}

const fn prerequisites(stage: Stage) -> &'static [Stage] {
    match stage {
        Stage::Detection => &[],
        Stage::Ocr | Stage::Inpainting => &[Stage::Detection],
        Stage::Summary => &[Stage::Detection, Stage::Ocr],
        Stage::Translation => &[Stage::Ocr, Stage::Summary],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(count: usize) -> Vec<EntityId> {
        (0..count).map(|_| EntityId::new()).collect()
    }

    #[test]
    fn starts_pages_in_order_and_models_independently() {
        let pages = pages(2);
        let mut scheduler = Scheduler::new(&pages, &Stage::ALL, None);
        let mut busy = BTreeSet::new();

        let first = scheduler.start_next(&busy).unwrap();
        assert_eq!(first, (pages[0], Stage::Detection));
        busy.insert(Stage::Detection);
        assert!(scheduler.start_next(&busy).is_none());

        busy.clear();
        assert!(!scheduler.complete_stage(pages[0], Stage::Detection));
        let ocr = scheduler.start_next(&busy).unwrap();
        busy.insert(ocr.1);
        let inpainting = scheduler.start_next(&busy).unwrap();
        busy.insert(inpainting.1);
        let next_page = scheduler.start_next(&busy).unwrap();
        busy.insert(next_page.1);
        assert_eq!(ocr, (pages[0], Stage::Ocr));
        assert_eq!(inpainting, (pages[0], Stage::Inpainting));
        assert_eq!(next_page, (pages[1], Stage::Detection));

        assert!(!scheduler.complete_stage(pages[0], Stage::Ocr));
        busy.remove(&Stage::Ocr);
        let summary = scheduler.start_next(&busy).unwrap();
        assert_eq!(summary, (pages[0], Stage::Summary));
        busy.insert(summary.1);
        assert!(scheduler.start_next(&busy).is_none());

        assert!(!scheduler.complete_stage(pages[0], Stage::Summary));
        busy.remove(&Stage::Summary);
        let translation = scheduler.start_next(&busy).unwrap();
        assert_eq!(translation, (pages[0], Stage::Translation));
        assert!(busy.contains(&Stage::Detection));
        assert!(busy.contains(&Stage::Inpainting));
    }

    #[test]
    fn translation_waits_for_earlier_pages_so_it_can_read_them() {
        let pages = pages(2);
        let stages = [Stage::Summary, Stage::Translation];
        let mut scheduler = Scheduler::new(&pages, &stages, None);
        let mut busy = BTreeSet::new();

        assert_eq!(scheduler.start_next(&busy), Some((pages[0], Stage::Summary)));
        assert!(!scheduler.complete_stage(pages[0], Stage::Summary));
        assert_eq!(
            scheduler.start_next(&busy),
            Some((pages[0], Stage::Translation))
        );
        busy.insert(Stage::Translation);
        // Page 1 may describe its scene while page 0 translates, but it
        // cannot translate before page 0's translation is committed.
        assert_eq!(scheduler.start_next(&busy), Some((pages[1], Stage::Summary)));
        busy.clear();
        assert!(!scheduler.complete_stage(pages[1], Stage::Summary));
        assert!(scheduler.start_next(&busy).is_none());

        assert!(scheduler.complete_stage(pages[0], Stage::Translation));
        assert_eq!(
            scheduler.start_next(&busy),
            Some((pages[1], Stage::Translation))
        );
    }

    #[test]
    fn a_paced_batch_runs_one_page_then_waits() {
        let pages = pages(2);
        let stages = [Stage::Detection, Stage::Ocr];
        let mut scheduler =
            Scheduler::new(&pages, &stages, Some(Duration::from_secs(60)));
        let busy = BTreeSet::new();

        assert_eq!(scheduler.start_next(&busy), Some((pages[0], Stage::Detection)));
        assert!(!scheduler.complete_stage(pages[0], Stage::Detection));
        // The second page does not start alongside the first.
        assert_eq!(scheduler.start_next(&busy), Some((pages[0], Stage::Ocr)));
        assert!(scheduler.start_next(&busy).is_none());
        assert!(scheduler.waiting().is_none());

        assert!(scheduler.complete_stage(pages[0], Stage::Ocr));
        assert!(scheduler.start_next(&busy).is_none());
        let (next, resume_at) = scheduler.waiting().unwrap();
        assert_eq!(next, pages[1]);
        assert!(resume_at > Instant::now() + Duration::from_secs(55));

        scheduler.resume_at = Some(Instant::now());
        assert_eq!(scheduler.start_next(&busy), Some((pages[1], Stage::Detection)));
    }

    #[test]
    fn sliding_window_backpressures_fast_upstream_models() {
        let pages = pages(4);
        let stages = [Stage::Detection, Stage::Ocr, Stage::Inpainting];
        let mut scheduler = Scheduler::new(&pages, &stages, None);
        let mut busy = BTreeSet::new();

        assert_eq!(
            scheduler.start_next(&busy),
            Some((pages[0], Stage::Detection))
        );
        assert!(!scheduler.complete_stage(pages[0], Stage::Detection));
        let ocr = scheduler.start_next(&busy).unwrap();
        busy.insert(ocr.1);
        let inpainting = scheduler.start_next(&busy).unwrap();
        busy.insert(inpainting.1);

        for page in &pages[1..3] {
            assert_eq!(scheduler.start_next(&busy), Some((*page, Stage::Detection)));
            assert!(!scheduler.complete_stage(*page, Stage::Detection));
        }
        assert!(scheduler.start_next(&busy).is_none());

        assert!(!scheduler.complete_stage(pages[0], Stage::Ocr));
        assert!(scheduler.complete_stage(pages[0], Stage::Inpainting));
        busy.clear();
        assert_eq!(scheduler.start_next(&busy), Some((pages[1], Stage::Ocr)));
    }
}
