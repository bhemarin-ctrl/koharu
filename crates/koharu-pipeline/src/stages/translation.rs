use anyhow::Result;
use async_trait::async_trait;
use koharu_scene::{
    Authored, EntityId, LanguageTag, Origin, SceneSummary, Snapshot, SourceText, Translation,
};
use koharu_translator::{TranslationContext, TranslationRequest, Translator};

use crate::TranslationConfig;

use super::{StageInput, StageProcessor, finish, generation, page_sources};

const PRODUCER: &str = "dev.koharu.pipeline.translation";

/// How many earlier pages a translation reads for dialogue and scene memory.
const MEMORY_PAGES: usize = 3;

pub(super) struct Processor {
    config: TranslationConfig,
    translator: Translator,
}

impl Processor {
    pub(super) fn new(config: TranslationConfig, translator: Translator) -> Self {
        Self { config, translator }
    }
}

#[async_trait]
impl StageProcessor for Processor {
    fn model(&self) -> &'static str {
        Translator::model(&self.config.model)
    }

    fn unload(&self) -> bool {
        self.translator.unload()
    }

    async fn load(&self) -> Result<()> {
        self.translator.load_model(&self.config.model).await
    }

    async fn process(&self, input: StageInput) -> Result<koharu_scene::Patch> {
        let mut targets = Vec::new();
        if let Some(group) = input.scene.page(input.page)?.text_group()? {
            for layer in group.text_layers()? {
                if !input.contains_entity(layer.id())? {
                    continue;
                }
                let content = layer.content()?;
                let Some(source) = content.source()? else {
                    continue;
                };
                if !source.text.value.trim().is_empty() {
                    targets.push((content.id(), source.text.value));
                }
            }
        }
        let (previous_scenes, context) = page_memory(&input.scene, input.page)?;
        let mut request = TranslationRequest::new(
            targets.iter().map(|(_, source)| source.clone()),
            self.config.target_language,
        )
        .with_previous_scenes(previous_scenes)
        .with_context(context);
        if let Some(summary) = input.scene.component::<SceneSummary>(input.page)?
            && !summary.text.value.trim().is_empty()
        {
            request = request.with_scene(summary.text.value);
        }
        if let Some(instructions) = self.config.instructions.as_deref() {
            request = request.with_instructions(instructions);
        }
        if Translator::supports_vision(&self.config.model, &self.config.generation)
            && let Some(image) = input.images.get(&input.scene, input.page, "source").await?
        {
            request = request.with_image(image);
        }
        let (provider, translated) = self
            .translator
            .translate(&self.config.model, self.config.generation, request)
            .await?;
        let language = LanguageTag::new(self.config.target_language.tag())?;
        let generated = generation(PRODUCER, provider)?;
        let mut edit = input.scene.edit_as(generated.clone());
        for (entity, _) in &targets {
            edit.observe::<SourceText>(*entity)?;
            edit.observe::<Translation>(*entity)?;
        }
        for ((entity, source), text) in targets.into_iter().zip(translated) {
            if input
                .scene
                .component::<Translation>(entity)?
                .is_some_and(|value| matches!(value.text.origin, Origin::User))
            {
                continue;
            }
            let text = if source.trim() == "\u{2026}" {
                "\u{2026}".to_owned()
            } else {
                text
            };
            edit.set(
                entity,
                &Translation {
                    text: Authored::generated(text, generated.clone()),
                    language: Some(language.clone()),
                },
            )?;
        }
        finish(edit)
    }
}

/// Scene summaries and translated dialogue of the pages right before `page`,
/// oldest first, so consecutive pages read as one conversation.
fn page_memory(scene: &Snapshot, page: EntityId) -> Result<(Vec<String>, Vec<TranslationContext>)> {
    let pages = scene.pages().map(|page| page.id()).collect::<Vec<_>>();
    let Some(position) = pages.iter().position(|candidate| *candidate == page) else {
        return Ok((Vec::new(), Vec::new()));
    };
    let mut scenes = Vec::new();
    let mut context = Vec::new();
    for previous in &pages[position.saturating_sub(MEMORY_PAGES)..position] {
        if let Some(summary) = scene.component::<SceneSummary>(*previous)?
            && !summary.text.value.trim().is_empty()
        {
            scenes.push(summary.text.value);
        }
        for (content, source) in page_sources(scene, *previous)? {
            if let Some(translation) = scene.component::<Translation>(content)?
                && !translation.text.value.trim().is_empty()
            {
                context.push(TranslationContext::new(source, translation.text.value));
            }
        }
    }
    Ok((scenes, context))
}

#[cfg(test)]
mod tests {
    use koharu_scene::{At, PageDraft, Session, TextLayout, TextLayoutKind};

    use super::*;

    fn text(edit: &mut koharu_scene::Edit, page: EntityId, source: &str, translation: &str) {
        let content = edit.add_text_content(page, At::End).unwrap();
        edit.add_text_layer(
            page,
            At::End,
            content,
            &TextLayout {
                origin: Origin::User,
                kind: TextLayoutKind::Paragraph,
                angle_degrees: None,
            },
        )
        .unwrap();
        edit.set(
            content,
            &SourceText {
                text: Authored::user(source.to_owned()),
                language: None,
            },
        )
        .unwrap();
        if !translation.is_empty() {
            edit.set(
                content,
                &Translation {
                    text: Authored::user(translation.to_owned()),
                    language: None,
                },
            )
            .unwrap();
        }
    }

    #[tokio::test]
    async fn memory_reads_earlier_pages_in_reading_order() {
        let mut session = Session::memory().await.unwrap();
        let mut pages = Vec::new();
        let patch = session
            .snapshot()
            .patch(|edit| {
                for label in ["1", "2", "3"] {
                    pages.push(edit.add_page(PageDraft::new(label, 1.0, 1.0), At::End)?);
                }
                edit.set(
                    pages[0],
                    &SceneSummary {
                        text: Authored::user("A forest at night.".to_owned()),
                    },
                )?;
                text(edit, pages[0], "自分の生きた証を", "Proof that I lived...");
                text(edit, pages[0], "この世界に", "I want to leave it in this world!");
                text(edit, pages[1], "未翻訳", "");
                text(edit, pages[2], "今", "Now");
                Ok(())
            })
            .unwrap();
        let scene = session.commit(patch).await.unwrap().snapshot;

        let (scenes, context) = page_memory(&scene, pages[2]).unwrap();
        assert_eq!(scenes, ["A forest at night."]);
        assert_eq!(
            context,
            [
                TranslationContext::new("自分の生きた証を", "Proof that I lived..."),
                TranslationContext::new("この世界に", "I want to leave it in this world!"),
            ]
        );

        let (scenes, context) = page_memory(&scene, pages[0]).unwrap();
        assert!(scenes.is_empty() && context.is_empty());
    }
}
