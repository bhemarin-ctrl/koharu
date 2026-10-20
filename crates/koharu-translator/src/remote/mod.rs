mod caiyun;
mod claude;
mod deepl;
mod deepseek;
mod gemini;
mod google_cloud;
mod grok;
mod lm_studio;
mod minimax;
mod openai;
mod openai_compatible;
mod openrouter;

use std::time::Duration;

use anyhow::Context;
use futures::{FutureExt, future::BoxFuture, future::join_all};
use reqwest::{Client, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;

pub use caiyun::CaiyunConfig;
pub use claude::ClaudeConfig;
pub use deepl::DeepLConfig;
pub use deepseek::DeepSeekConfig;
pub use gemini::GeminiConfig;
pub use google_cloud::GoogleCloudConfig;
pub use grok::GrokConfig;
pub use lm_studio::LmStudioConfig;
pub use minimax::MiniMaxConfig;
pub use openai::OpenAiConfig;
pub use openai_compatible::OpenAiCompatibleConfig;
pub use openrouter::OpenRouterConfig;

use crate::{
    Error, GenerationConfig, Model, ModelSelection, Provider, ProvidersConfig, Result,
    TranslationRequest,
    prompt::{self, Completion},
};

pub(crate) async fn translate(
    client: &Client,
    providers: &ProvidersConfig,
    selection: &ModelSelection,
    generation: &GenerationConfig,
    request: &TranslationRequest,
) -> Result<Vec<String>> {
    match selection.provider {
        Provider::DeepL => deepl::translate(client, &providers.deepl, request).await,
        Provider::GoogleCloudTranslation => {
            google_cloud::translate(client, &providers.google_cloud_translation, request).await
        }
        Provider::Caiyun => caiyun::translate(client, &providers.caiyun, request).await,
        provider => {
            let completion = prompt::translation(request)?;
            let text = complete(client, providers, selection, generation, &completion).await?;
            Ok(prompt::translations(
                provider.into(),
                &text,
                &request.segments,
            )?)
        }
    }
}

/// Sends one structured-output request to a hosted language model and
/// returns the raw JSON text. Machine-translation services cannot answer one.
pub(crate) async fn complete(
    client: &Client,
    providers: &ProvidersConfig,
    selection: &ModelSelection,
    generation: &GenerationConfig,
    completion: &Completion,
) -> Result<String> {
    let model = || {
        selection
            .model
            .as_deref()
            .with_context(|| format!("{} requires a selected model", selection.provider))
    };
    match selection.provider {
        Provider::OpenAi => {
            openai::complete(client, &providers.openai, model()?, generation, completion).await
        }
        Provider::Gemini => {
            gemini::complete(client, &providers.gemini, model()?, generation, completion).await
        }
        Provider::Claude => {
            claude::complete(client, &providers.claude, model()?, generation, completion).await
        }
        Provider::Grok => {
            grok::complete(client, &providers.grok, model()?, generation, completion).await
        }
        Provider::MiniMax => {
            minimax::complete(client, &providers.minimax, model()?, generation, completion).await
        }
        Provider::DeepSeek => {
            deepseek::complete(
                client,
                &providers.deepseek,
                model()?,
                generation,
                completion,
            )
            .await
        }
        Provider::OpenAiCompatible => {
            openai_compatible::complete(
                client,
                &providers.openai_compatible,
                model()?,
                generation,
                completion,
            )
            .await
        }
        Provider::OpenRouter => {
            openrouter::complete(
                client,
                &providers.openrouter,
                model()?,
                generation,
                completion,
            )
            .await
        }
        Provider::LmStudio => {
            lm_studio::complete(
                client,
                &providers.lm_studio,
                model()?,
                generation,
                completion,
            )
            .await
        }
        Provider::DeepL | Provider::GoogleCloudTranslation | Provider::Caiyun => {
            Err(Error::PromptUnsupported {
                provider: selection.provider.into(),
            })
        }
        Provider::Local => unreachable!("local models have their own backend"),
    }
}

pub(crate) async fn models(client: &Client, providers: &ProvidersConfig) -> Vec<Model> {
    let mut models = Vec::new();
    let pending: Vec<BoxFuture<'_, Result<Vec<Model>>>> = vec![
        openai::models(client).boxed(),
        gemini::models(client).boxed(),
        claude::models(client).boxed(),
        grok::models(client).boxed(),
        minimax::models(client).boxed(),
        deepseek::models(client).boxed(),
        openai_compatible::models(client, &providers.openai_compatible).boxed(),
        openrouter::models(client).boxed(),
        lm_studio::models(client, &providers.lm_studio).boxed(),
        deepl::models().boxed(),
        google_cloud::models().boxed(),
        caiyun::models().boxed(),
    ];
    for result in join_all(pending).await {
        append_models(&mut models, result);
    }
    models.sort_by(|left, right| {
        left.provider
            .to_string()
            .cmp(&right.provider.to_string())
            .then_with(|| left.model.cmp(&right.model))
    });
    models.dedup_by(|left, right| left.provider == right.provider && left.model == right.model);
    models
}

fn append_models(models: &mut Vec<Model>, result: Result<Vec<Model>>) {
    match result {
        Ok(mut provider_models) => models.append(&mut provider_models),
        Err(error) => tracing::warn!(%error, "failed to list translation models"),
    }
}

/// Waits before each retry of a transient failure. Hosted models answer
/// overload and per-minute rate limits with 429, 500, 502, 503 and 504, which
/// usually clear within a minute; without retrying, one such answer would stop
/// a long batch partway through a volume.
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(60),
];

pub(super) async fn send_json<T: DeserializeOwned>(
    provider: &'static str,
    request: RequestBuilder,
) -> Result<T> {
    let mut request = request;
    let mut delays = RETRY_DELAYS.iter();
    loop {
        let retry = request.try_clone();
        let (result, transient) = send_once(provider, request).await;
        match (result, transient, retry, delays.next()) {
            (Err(error), true, Some(next), Some(delay)) => {
                tracing::warn!(provider, %error, ?delay, "retrying after a transient model API failure");
                tokio::time::sleep(*delay).await;
                request = next;
            }
            (result, ..) => return result,
        }
    }
}

/// Sends one request. The flag reports whether a failure is worth retrying:
/// a lost connection, a timeout, an overloaded server, or a rate limit, but
/// not an exhausted balance, a bad key, or a malformed request.
async fn send_once<T: DeserializeOwned>(
    provider: &'static str,
    request: RequestBuilder,
) -> (Result<T>, bool) {
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            let transient = error.is_timeout() || error.is_connect() || error.is_request();
            let error = anyhow::Error::new(error).context(format!("{provider} request failed"));
            return (Err(error.into()), transient);
        }
    };
    let status = response.status();
    let text = match response.text().await {
        Ok(text) => text,
        Err(error) => {
            let error =
                anyhow::Error::new(error).context(format!("failed to read {provider} response"));
            return (Err(error.into()), true);
        }
    };

    if !status.is_success() {
        let lower = text.to_ascii_lowercase();
        let exhausted =
            lower.contains("insufficient_quota") || lower.contains("credit balance is too low");
        if status == StatusCode::TOO_MANY_REQUESTS
            || exhausted
            || lower.contains("resource_exhausted")
            || lower.contains("rate limit exceeded")
        {
            return (Err(Error::QuotaExceeded { provider }), !exhausted);
        }
        let transient = matches!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR
                | StatusCode::BAD_GATEWAY
                | StatusCode::SERVICE_UNAVAILABLE
                | StatusCode::GATEWAY_TIMEOUT
        );
        return (
            Err(Error::Api {
                provider,
                status: status.as_u16(),
                message: text.chars().take(4096).collect(),
            }),
            transient,
        );
    }

    let decoded = serde_json::from_str(&text)
        .with_context(|| format!("failed to decode {provider} response"))
        .map_err(Into::into);
    (decoded, false)
}
