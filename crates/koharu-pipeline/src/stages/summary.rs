use anyhow::Result;
use async_trait::async_trait;
use koharu_scene::{Authored, Origin, SceneSummary};
use koharu_translator::{ModelSelection, SummaryRequest, Translator};

use crate::{SummaryConfig, TranslationConfig};

use super::{StageInput, StageProcessor, finish, generation, page_sources, previous_page};

const PRODUCER: &str = "dev.koharu.pipeline.summary";

/// Describes each page with a vision model so translation knows who speaks,
/// what happens, and how the story continues from the previous page.
pub(super) struct Processor {
    summary: SummaryConfig,
    translation: TranslationConfig,
    translator: Translator,
}

impl Processor {
    pub(super) fn new(
        summary: SummaryConfig,
        translation: TranslationConfig,
        translator: Translator,
    ) -> Self {
        Self {
            summary,
            translation,
            translator,
        }
    }

    fn selection(&self) -> &ModelSelection {
        self.summary.model(&self.translation)
    }
}

#[async_trait]
impl StageProcessor for Processor {
    fn model(&self) -> &'static str {
        Translator::model(self.selection())
    }

    fn skip(&self, input: &StageInput) -> Result<bool> {
        // A model without image input cannot see the page, and a summary the
        // user wrote is kept the same way user translations are.
        if !Translator::supports_vision(self.selection(), &self.translation.generation) {
            return Ok(true);
        }
        Ok(input
            .scene
            .component::<SceneSummary>(input.page)?
            .is_some_and(|summary| matches!(summary.text.origin, Origin::User)))
    }

    fn unload(&self) -> bool {
        self.translator.unload()
    }

    async fn load(&self) -> Result<()> {
        self.translator.load_model(self.selection()).await
    }

    async fn process(&self, input: StageInput) -> Result<koharu_scene::Patch> {
        let Some(image) = input.images.get(&input.scene, input.page, "source").await? else {
            return finish(input.scene.edit());
        };
        let segments = page_sources(&input.scene, input.page)?
            .into_iter()
            .map(|(_, source)| source);
        let mut request =
            SummaryRequest::new(image, self.translation.target_language).with_segments(segments);
        if let Some(previous) = previous_page(&input.scene, input.page)
            && let Some(summary) = input.scene.component::<SceneSummary>(previous)?
            && !summary.text.value.trim().is_empty()
        {
            request = request.with_previous_scene(summary.text.value);
        }
        if let Some(instructions) = self.translation.instructions.as_deref() {
            request = request.with_instructions(instructions);
        }
        let (provider, text) = self
            .translator
            .summarize(self.selection(), self.translation.generation, request)
            .await?;
        let generated = generation(PRODUCER, provider)?;
        let mut edit = input.scene.edit_as(generated.clone());
        edit.observe::<SceneSummary>(input.page)?;
        edit.set(
            input.page,
            &SceneSummary {
                text: Authored::generated(text, generated),
            },
        )?;
        finish(edit)
    }
}
