use std::{fmt::Write, sync::Arc};

use anyhow::Context;
use image::DynamicImage;
use indoc::indoc;
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Value, json};

use crate::{Language, SummaryRequest, TranslationContext, TranslationRequest};

/// One structured-output request to a language model. Every LLM provider
/// receives this shape, so translation and scene summaries share the same
/// provider code path and the same native structured-output enforcement.
pub(crate) struct Completion {
    pub(crate) name: &'static str,
    pub(crate) system: String,
    pub(crate) user: String,
    pub(crate) image: Option<Arc<DynamicImage>>,
    pub(crate) schema: Value,
}

pub(crate) fn translation(request: &TranslationRequest) -> anyhow::Result<Completion> {
    let input = TranslationInput {
        source_language: request.source_language,
        target_language: request.target_language,
        previous_scenes: &request.previous_scenes,
        context: &request.context,
        scene: request.scene.as_deref(),
        segments: request
            .segments
            .iter()
            .enumerate()
            .map(|(id, text)| TranslationInputSegment { id, text })
            .collect(),
    };
    let user = serde_json::to_string(&input).context("failed to serialize translation input")?;
    Ok(Completion {
        name: "manga_translation",
        system: translation_system_prompt(request),
        user,
        image: request.image.clone(),
        schema: output_schema(request.segments.len()),
    })
}

pub(crate) fn summary(request: &SummaryRequest) -> anyhow::Result<Completion> {
    let input = SummaryInput {
        previous_scene: request.previous_scene.as_deref(),
        segments: request
            .segments
            .iter()
            .enumerate()
            .map(|(id, text)| TranslationInputSegment { id, text })
            .collect(),
    };
    let user = serde_json::to_string(&input).context("failed to serialize summary input")?;
    Ok(Completion {
        name: "manga_scene_summary",
        system: summary_system_prompt(request),
        user,
        image: Some(Arc::clone(&request.image)),
        schema: summary_schema(),
    })
}

pub(crate) fn translations(
    provider: &str,
    text: &str,
    source_segments: &[String],
) -> anyhow::Result<Vec<String>> {
    let output = serde_json::from_str::<TranslationOutput>(text).with_context(|| {
        format!(
            "{provider} returned invalid translation JSON for {} segments; response was: {}",
            source_segments.len(),
            snippet(text),
        )
    })?;
    let mut translations = source_segments.to_vec();
    let mut translated = vec![false; source_segments.len()];

    for translation in output.translations {
        if translation.id < translations.len() && !translated[translation.id] {
            translations[translation.id] = translation.text;
            translated[translation.id] = true;
        }
    }

    Ok(translations)
}

pub(crate) fn summary_text(provider: &str, text: &str) -> anyhow::Result<String> {
    let output = serde_json::from_str::<SummaryOutput>(text).with_context(|| {
        format!(
            "{provider} returned invalid scene summary JSON; response was: {}",
            snippet(text),
        )
    })?;
    let summary = output.summary.trim();
    anyhow::ensure!(
        !summary.is_empty(),
        "{provider} returned an empty scene summary"
    );
    Ok(summary.to_owned())
}

/// Keeps a model response readable in a log line without truncating so hard
/// that the shape of the failure is lost.
///
/// Responses are usually pretty-printed, so line breaks and other control
/// characters are escaped rather than passed through: the snippet is formatted
/// into an error context, and one failure should stay one line.
pub(crate) fn snippet(text: &str) -> String {
    const LIMIT: usize = 2000;
    let trimmed = text.trim();
    let (visible, elided) = match trimmed.char_indices().nth(LIMIT) {
        Some((end, _)) => (&trimmed[..end], true),
        None => (trimmed, false),
    };

    let mut output = String::with_capacity(visible.len());
    for character in visible.chars() {
        match character {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            control if control.is_control() => {
                let _ = write!(output, "\\u{{{:04x}}}", control as u32);
            }
            character => output.push(character),
        }
    }
    if elided {
        let _ = write!(output, "… ({} bytes total)", trimmed.len());
    }
    output
}

pub(crate) fn output_schema(expected: usize) -> Value {
    json!({
        "type": "object",
        "properties": {
            "translations": {
                "type": "array",
                "minItems": expected,
                "maxItems": expected,
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": expected.saturating_sub(1),
                            "description": "The ID copied from the corresponding input segment."
                        },
                        "text": {
                            "type": "string",
                            "description": "The translation of the input segment with this ID."
                        }
                    },
                    "required": ["id", "text"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["translations"],
        "additionalProperties": false
    })
}

fn summary_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": {
                "type": "string",
                "description": "What happens on the page, in prose."
            }
        },
        "required": ["summary"],
        "additionalProperties": false
    })
}

fn translation_system_prompt(request: &TranslationRequest) -> String {
    let source = request
        .source_language
        .map(|language| language.to_string())
        .unwrap_or_else(|| "the detected source language".to_owned());
    let mut prompt = format!(
        indoc! {"
            You are a professional manga translator.

            Translation requirements:
            - Translate every input segment from {source} into natural {target}.
            - Preserve meaning, character voice, emotional tone, relationship nuance, emphasis, and sound effects.
            - Localize idioms and sound effects naturally while keeping wording concise enough for speech bubbles.
            - Use surrounding segments only for disambiguation and continuity; never merge or split segments.
            - Segments are listed in reading order, one per speech bubble or caption. A sentence may run across consecutive segments; translate each part so the split reads naturally in {target}.
            - Write every translated `text` value only in {target}; do not include source text, notes, explanations, or alternatives.
            - Never preserve or repeat original-language text; translate names, terms, and sound effects using natural {target} conventions.

            Output requirements:
            - Each input segment has a numeric `id`.
            - Return only a JSON object whose `translations` array contains one object with `id` and translated `text` for every input segment.
            - Copy every input ID exactly once; order does not matter.
            - Never merge, split, omit, duplicate, or add segments.
        "},
        source = source,
        target = request.target_language,
    )
    .trim_end()
    .to_owned();

    if !request.context.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(indoc! {"
            Context requirements:
            The `context` entries are dialogue already translated on the previous pages, oldest first.
            Use them only to keep names, terms, forms of address, character voice, and dialogue continuity consistent.
            Do not translate or return the context entries.
        "}.trim_end());
    }

    if request.scene.is_some() || !request.previous_scenes.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(indoc! {"
            Scene requirements:
            `scene` describes what happens on this page and may refer to segments by their `id`; `previous_scenes` describe the previous pages, oldest first.
            Use them to identify who is speaking or thinking, their tone, and how the story continues.
            Do not translate or return the scene descriptions.
        "}.trim_end());
    }

    if request.image.is_some() {
        prompt.push_str("\n\n");
        prompt.push_str(indoc! {"
            Image requirements:
            Use the attached original page image as visual context for speaker identity, tone, layout, and ambiguous OCR.
            Translate only the supplied segments; do not add text seen in the image that is absent from the input segments.
        "}.trim_end());
    }

    if let Some(instructions) = request
        .instructions
        .as_deref()
        .map(str::trim)
        .filter(|instructions| !instructions.is_empty())
    {
        prompt.push_str("\n\nAdditional instructions:\n");
        prompt.push_str(instructions);
    }
    prompt
}

fn summary_system_prompt(request: &SummaryRequest) -> String {
    let mut prompt = format!(
        indoc! {"
            You describe manga pages for a translator who will translate the page's text segments.

            Summary requirements:
            - Write in {language}, in two to five sentences of plain prose.
            - Describe the setting, who appears, what happens in reading order, and the mood.
            - When `segments` are supplied, say who most likely says or thinks each one, referring to segments by their `id`, and whether a segment is speech, thought, narration, or a sound effect.
            - `previous_scene` describes the previous page; keep names and roles consistent with it.
            - Name a character only when the name appears in the segments, the previous scene, or the additional instructions; otherwise describe them, for example \"the blond boy\".
            - Do not translate the segments line by line and do not add notes, lists, headings, or alternatives.

            Output requirements:
            - Return only a JSON object with a `summary` string.
        "},
        language = request.language,
    )
    .trim_end()
    .to_owned();

    if let Some(instructions) = request
        .instructions
        .as_deref()
        .map(str::trim)
        .filter(|instructions| !instructions.is_empty())
    {
        prompt.push_str("\n\nAdditional instructions for this work:\n");
        prompt.push_str(instructions);
    }
    prompt
}

#[derive(Serialize)]
struct TranslationInput<'a> {
    source_language: Option<Language>,
    target_language: Language,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    previous_scenes: &'a [String],
    context: &'a [TranslationContext],
    #[serde(skip_serializing_if = "Option::is_none")]
    scene: Option<&'a str>,
    segments: Vec<TranslationInputSegment<'a>>,
}

#[derive(Serialize)]
struct SummaryInput<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_scene: Option<&'a str>,
    segments: Vec<TranslationInputSegment<'a>>,
}

#[derive(Debug, Deserialize)]
struct SummaryOutput {
    summary: String,
}

#[derive(Serialize)]
struct TranslationInputSegment<'a> {
    id: usize,
    text: &'a str,
}

#[derive(Debug, Deserialize)]
struct TranslationOutput {
    translations: Vec<TranslationOutputSegment>,
}

#[derive(Debug, Deserialize)]
struct TranslationOutputSegment {
    #[serde(deserialize_with = "deserialize_segment_id")]
    id: usize,
    text: String,
}

fn deserialize_segment_id<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum SegmentId {
        Number(usize),
        String(String),
    }

    match SegmentId::deserialize(deserializer)? {
        SegmentId::Number(id) => Ok(id),
        SegmentId::String(id) => id.trim().parse().map_err(de::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_keeps_a_response_on_one_line() {
        let flattened = snippet("{\n  \"translations\": [\n\t\"hello\"\n  ]\n}");

        assert!(!flattened.contains('\n'), "{flattened}");
        assert!(!flattened.contains('\t'), "{flattened}");
        assert_eq!(flattened, r#"{\n  "translations": [\n\t"hello"\n  ]\n}"#);
    }

    #[test]
    fn snippet_escapes_other_control_characters() {
        assert_eq!(snippet("before\u{7}after"), r"before\u{0007}after");
    }

    #[test]
    fn snippet_reports_the_length_it_elided() {
        let flattened = snippet(&"x".repeat(2_500));

        assert!(flattened.starts_with(&"x".repeat(2_000)));
        assert!(
            flattened.ends_with("\u{2026} (2500 bytes total)"),
            "{flattened}"
        );
    }

    #[test]
    fn parses_plain_json() {
        let source = ["one".to_owned(), "two".to_owned()];
        let response = r#"{"translations":[{"id":0,"text":"hello"},{"id":1,"text":"world"}]}"#;

        assert_eq!(
            translations("test", response, &source).unwrap(),
            ["hello", "world"]
        );
    }

    #[test]
    fn rejects_wrapped_and_malformed_json() {
        let source = ["one".to_owned(), "two".to_owned()];
        for response in [
            "```json\n{\"translations\":[{\"id\":0,\"text\":\"hello\"},{\"id\":1,\"text\":\"world\"}]}\n```",
            r#"{translations: [{id: 0, text: 'hello'}, {id: 1, text: 'world'},],}"#,
            r#"Here is the result: {"translations": [{"id": 0, "text": "hello"}, {"id": 1, "text": "world"},]}"#,
            "{\"translations\":[{\"id\":0,\"text\":\"hello\"},{\"id\":1,\"text\":\"world\"",
        ] {
            assert!(
                translations("test", response, &source).is_err(),
                "{response}"
            );
        }
    }

    #[test]
    fn parse_error_preserves_the_root_failure_and_response() {
        let source = ["one".to_owned()];
        let response = "{\n  \"translations\": [{\"id\": 0}]\n}";
        let error = format!(
            "{:#}",
            translations("test", response, &source).expect_err("missing text should fail")
        );

        assert!(error.contains("missing field `text`"), "{error}");
        assert!(
            error.contains(r#"response was: {\n  "translations": [{"id": 0}]\n}"#),
            "{error}"
        );
    }

    #[test]
    fn restores_input_order_from_ids() {
        let source = ["one".to_owned(), "two".to_owned()];
        let response = r#"{"translations":[{"id":1,"text":"world"},{"id":0,"text":"hello"}]}"#;
        assert_eq!(
            translations("test", response, &source).unwrap(),
            ["hello", "world"]
        );
    }

    #[test]
    fn tolerates_duplicate_missing_and_out_of_range_ids() {
        let source = ["one".to_owned(), "two".to_owned()];
        let short = r#"{"translations":[{"id":1,"text":"world"}]}"#;
        assert_eq!(
            translations("test", short, &source).unwrap(),
            ["one", "world"]
        );

        let response = concat!(
            r#"{"translations":["#,
            r#"{"id":0,"text":"hello"},"#,
            r#"{"id":0,"text":"duplicate"},"#,
            r#"{"id":9,"text":"extra"}"#,
            "]}"
        );
        assert_eq!(
            translations("test", response, &source).unwrap(),
            ["hello", "two"]
        );
    }

    #[test]
    fn prompt_payload_contains_ordered_context() {
        let request = TranslationRequest::new(["new"], Language::English)
            .with_context([TranslationContext::new("old", "previous")]);
        let completion = translation(&request).unwrap();
        let input: serde_json::Value = serde_json::from_str(&completion.user).unwrap();
        assert_eq!(input["context"][0]["source"], "old");
        assert_eq!(input["context"][0]["translation"], "previous");
        assert_eq!(input["segments"][0]["id"], 0);
        assert_eq!(input["segments"][0]["text"], "new");
    }

    #[test]
    fn system_prompt_encodes_invariants_and_custom_instructions() {
        let request = TranslationRequest::new(["hello"], Language::Korean)
            .with_source_language(Language::Japanese)
            .with_instructions("Use informal speech.");
        let prompt = translation_system_prompt(&request);
        assert!(prompt.contains("from Japanese into natural Korean"));
        assert!(prompt.contains("Copy every input ID exactly once"));
        assert!(prompt.contains("Use informal speech."));
    }

    #[test]
    fn schema_requires_the_expected_number_of_id_text_pairs() {
        let schema = output_schema(3);
        let translations = &schema["properties"]["translations"];
        assert_eq!(translations["minItems"], 3);
        assert_eq!(translations["maxItems"], 3);
        assert_eq!(translations["items"]["properties"]["id"]["minimum"], 0);
        assert_eq!(translations["items"]["properties"]["id"]["maximum"], 2);
        assert_eq!(translations["items"]["additionalProperties"], false);
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn empty_custom_instructions_are_ignored() {
        let request = TranslationRequest::new(["hello"], Language::English).with_instructions("  ");
        assert!(!translation_system_prompt(&request).contains("Additional instructions"));
    }

    #[test]
    fn context_is_reference_only() {
        let request = TranslationRequest::new(["Where is she?"], Language::Japanese)
            .with_context([TranslationContext::new("I saw Alice.", "アリスを見た。")]);
        let prompt = translation_system_prompt(&request);
        assert!(prompt.contains("dialogue continuity"));
        assert!(prompt.contains("Do not translate or return the context"));
    }

    #[test]
    fn page_memory_reaches_the_payload_and_the_rules() {
        let request = TranslationRequest::new(["この世界に"], Language::English)
            .with_context([TranslationContext::new("前の台詞", "earlier line")])
            .with_scene("Segment 0 is the boy's inner thought.")
            .with_previous_scenes(["The boy enters a forest.".to_owned()]);
        let completion = translation(&request).unwrap();
        let input: serde_json::Value = serde_json::from_str(&completion.user).unwrap();
        assert_eq!(input["scene"], "Segment 0 is the boy's inner thought.");
        assert_eq!(input["previous_scenes"][0], "The boy enters a forest.");
        assert_eq!(input["context"][0]["translation"], "earlier line");
        assert!(completion.system.contains("Scene requirements"));
        assert!(
            completion
                .system
                .contains("Do not translate or return the scene")
        );
    }

    #[test]
    fn a_page_without_memory_sends_no_scene_fields() {
        let completion =
            translation(&TranslationRequest::new(["text"], Language::English)).unwrap();
        let input: serde_json::Value = serde_json::from_str(&completion.user).unwrap();
        assert!(input.get("scene").is_none());
        assert!(input.get("previous_scenes").is_none());
        assert!(!completion.system.contains("Scene requirements"));
    }

    #[test]
    fn summary_requests_the_image_and_a_single_summary_field() {
        let request = SummaryRequest::new(
            std::sync::Arc::new(image::DynamicImage::new_rgb8(1, 1)),
            Language::English,
        )
        .with_segments(["自分の生きた証を"])
        .with_previous_scene("The boy faces wolves.")
        .with_instructions("Never keep honorifics.");
        let completion = summary(&request).unwrap();
        assert!(completion.image.is_some());
        assert_eq!(completion.schema["required"][0], "summary");
        assert_eq!(completion.schema["additionalProperties"], false);
        let input: serde_json::Value = serde_json::from_str(&completion.user).unwrap();
        assert_eq!(input["segments"][0]["id"], 0);
        assert_eq!(input["previous_scene"], "The boy faces wolves.");
        assert!(completion.system.contains("Never keep honorifics."));
    }

    #[test]
    fn summary_output_fails_closed() {
        assert_eq!(
            summary_text("test", r#"{"summary":"  A forest.  "}"#).unwrap(),
            "A forest."
        );
        assert!(summary_text("test", r#"{"summary":"   "}"#).is_err());
        assert!(summary_text("test", "A forest.").is_err());
    }

    #[test]
    fn image_context_does_not_expand_the_translation_scope() {
        let request = TranslationRequest::new(["text"], Language::English)
            .with_image(std::sync::Arc::new(image::DynamicImage::new_rgb8(1, 1)));
        let prompt = translation_system_prompt(&request);
        assert!(prompt.contains("attached original page image"));
        assert!(prompt.contains("Translate only the supplied segments"));
    }
}
