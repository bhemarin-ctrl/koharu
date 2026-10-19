use std::{io::Cursor, sync::Arc};

use anyhow::Context;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use fast_image_resize::{FilterType, ResizeAlg, ResizeOptions, Resizer};
use image::{DynamicImage, ImageEncoder, RgbImage, codecs::jpeg::JpegEncoder};

use crate::Language;

const MAX_IMAGE_DIMENSION: u32 = 2048;
const JPEG_QUALITY: u8 = 88;

#[derive(Debug, Clone, PartialEq)]
pub struct TranslationRequest {
    pub segments: Vec<String>,
    pub source_language: Option<Language>,
    pub target_language: Language,
    pub instructions: Option<String>,
    /// Dialogue already translated on the pages before this one, oldest first.
    pub context: Vec<TranslationContext>,
    /// What happens on this page, written by the scene-summary step.
    pub scene: Option<String>,
    /// Scene summaries of the pages before this one, oldest first.
    pub previous_scenes: Vec<String>,
    pub image: Option<Arc<DynamicImage>>,
}

impl TranslationRequest {
    #[must_use]
    pub fn new(
        segments: impl IntoIterator<Item = impl Into<String>>,
        target_language: Language,
    ) -> Self {
        Self {
            segments: segments.into_iter().map(Into::into).collect(),
            source_language: None,
            target_language,
            instructions: None,
            context: Vec::new(),
            scene: None,
            previous_scenes: Vec::new(),
            image: None,
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_source_language(mut self, language: Language) -> Self {
        self.source_language = Some(language);
        self
    }

    #[must_use]
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    #[must_use]
    pub fn with_image(mut self, image: Arc<DynamicImage>) -> Self {
        self.image = Some(image);
        self
    }

    #[must_use]
    pub fn with_context(mut self, context: impl IntoIterator<Item = TranslationContext>) -> Self {
        self.context = context.into_iter().collect();
        self
    }

    #[must_use]
    pub fn with_scene(mut self, scene: impl Into<String>) -> Self {
        self.scene = Some(scene.into());
        self
    }

    #[must_use]
    pub fn with_previous_scenes(mut self, scenes: impl IntoIterator<Item = String>) -> Self {
        self.previous_scenes = scenes.into_iter().collect();
        self
    }

    pub(crate) fn prepare_image(&mut self) -> anyhow::Result<()> {
        if let Some(image) = self.image.as_ref() {
            self.image = Some(fit_image(image)?);
        }
        Ok(())
    }

    pub(crate) fn remove_image(&mut self) {
        self.image = None;
    }
}

/// Asks a vision model what happens on one page, so that translation can
/// identify speakers and keep the story continuous.
#[derive(Debug, Clone, PartialEq)]
pub struct SummaryRequest {
    pub image: Arc<DynamicImage>,
    /// The page's source text in reading order, when it has been read already.
    pub segments: Vec<String>,
    pub language: Language,
    pub instructions: Option<String>,
    /// The scene summary of the previous page.
    pub previous_scene: Option<String>,
}

impl SummaryRequest {
    #[must_use]
    pub fn new(image: Arc<DynamicImage>, language: Language) -> Self {
        Self {
            image,
            segments: Vec::new(),
            language,
            instructions: None,
            previous_scene: None,
        }
    }

    #[must_use]
    pub fn with_segments(mut self, segments: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.segments = segments.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    #[must_use]
    pub fn with_previous_scene(mut self, scene: impl Into<String>) -> Self {
        self.previous_scene = Some(scene.into());
        self
    }

    pub(crate) fn prepare_image(&mut self) -> anyhow::Result<()> {
        self.image = fit_image(&self.image)?;
        Ok(())
    }
}

fn fit_image(image: &Arc<DynamicImage>) -> anyhow::Result<Arc<DynamicImage>> {
    let (width, height) = (image.width(), image.height());
    let longest = width.max(height);
    if longest <= MAX_IMAGE_DIMENSION {
        return Ok(Arc::clone(image));
    }
    let resized_width =
        (u64::from(width) * u64::from(MAX_IMAGE_DIMENSION) / u64::from(longest)).max(1) as u32;
    let resized_height =
        (u64::from(height) * u64::from(MAX_IMAGE_DIMENSION) / u64::from(longest)).max(1) as u32;
    let source = image.to_rgb8();
    let mut resized = RgbImage::new(resized_width, resized_height);
    Resizer::new()
        .resize(
            &source,
            &mut resized,
            &ResizeOptions::new()
                .resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3))
                .use_alpha(false),
        )
        .context("failed to resize page image")?;
    Ok(Arc::new(DynamicImage::ImageRgb8(resized)))
}

pub(crate) struct EncodedImage {
    pub(crate) data: String,
}

impl EncodedImage {
    pub(crate) fn data_url(&self) -> String {
        format!("data:image/jpeg;base64,{}", self.data)
    }
}

pub(crate) fn encode_image(image: &DynamicImage) -> anyhow::Result<EncodedImage> {
    let rgb = image.to_rgb8();
    let mut bytes = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY)
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .context("failed to encode translation image")?;
    Ok(EncodedImage {
        data: STANDARD.encode(bytes.into_inner()),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TranslationContext {
    pub source: String,
    pub translation: String,
}

impl TranslationContext {
    #[must_use]
    pub fn new(source: impl Into<String>, translation: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            translation: translation.into(),
        }
    }
}
