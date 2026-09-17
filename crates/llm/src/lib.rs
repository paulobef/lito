//! LLM Client - Ollama integration with streaming support

use std::time::{Duration, Instant};

use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;
use tracing::debug;

/// Ollama's own default listen address - used as the fallback when no
/// endpoint has been persisted yet (first launch, or a fresh settings
/// reset), and as the "default" value shown/restored in the settings UI.
pub const DEFAULT_BASE_URL: &str = "http://localhost:11434";

/// Ollama client for LLM interactions
pub struct OllamaClient {
    client: Client,
    base_url: String,
    model: String,
}

#[derive(Debug, Serialize)]
struct GenerateRequest {
    model: String,
    prompt: String,
    system: Option<String>,
    stream: bool,
    format: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GenerateResponse {
    response: String,
    // Present in Ollama's response shape; not yet consulted since the byte
    // stream already ends when the HTTP body closes.
    #[allow(dead_code)]
    done: bool,
}

/// A single turn in a conversation, for the `/api/chat` endpoint. `role` is
/// one of "system", "user", "assistant" (Ollama's chat role vocabulary).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    // Reasoning-trained models (qwen3, deepseek-r1, qwq, ...) return their
    // reasoning trace in a separate `thinking` field on the response message
    // when this is set. Non-reasoning models silently ignore it - no error,
    // no `thinking` field in the response - so it's safe to always send.
    think: bool,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    message: ChatResponseMessage,
    // Same story as `GenerateResponse::done`: the HTTP body closing already
    // tells us the stream is over.
    #[allow(dead_code)]
    done: bool,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    content: String,
    #[serde(default)]
    thinking: Option<String>,
}

/// One piece of streamed output from `/api/chat`: either a chunk of the
/// model's visible answer, or - for a reasoning-trained model with `think`
/// enabled - a chunk of its reasoning trace. Kept as separate variants so
/// callers can route each to a different UI surface instead of having to
/// re-inspect the source line.
#[derive(Debug, Clone)]
pub enum ChatChunk {
    Thinking(String),
    Content(String),
}

#[derive(Debug, Deserialize)]
struct TagsResponse {
    models: Vec<TagsModel>,
}

#[derive(Debug, Deserialize)]
struct TagsModel {
    name: String,
}

impl OllamaClient {
    /// Create a new Ollama client
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into(),
            model: model.into(),
        }
    }

    /// Get the current model name
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Switch which model subsequent requests use
    pub fn set_model(&mut self, model: impl Into<String>) {
        self.model = model.into();
    }

    /// Get the current Ollama endpoint (e.g. `http://localhost:11434`).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Point subsequent requests at a different Ollama endpoint - for a
    /// server exposed on a non-default host/port (a remote machine, a
    /// forwarded port, a container).
    pub fn set_base_url(&mut self, base_url: impl Into<String>) {
        self.base_url = base_url.into();
    }

    /// List model names currently pulled in Ollama
    pub async fn list_models(&self) -> Result<Vec<String>> {
        let response = self
            .client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await?
            .json::<TagsResponse>()
            .await?;

        Ok(response.models.into_iter().map(|m| m.name).collect())
    }

    /// Check if Ollama is available
    pub async fn health_check(&self) -> bool {
        self.client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    /// Generate a completion (non-streaming)
    pub async fn complete(&self, system: &str, prompt: &str) -> Result<String> {
        let request = GenerateRequest {
            model: self.model.clone(),
            prompt: prompt.to_string(),
            system: Some(system.to_string()),
            stream: false,
            format: None,
        };

        debug!("Sending request to Ollama: {:?}", request);

        let response = self
            .client
            .post(format!("{}/api/generate", self.base_url))
            .json(&request)
            .send()
            .await?
            .json::<GenerateResponse>()
            .await?;

        Ok(response.response)
    }

    /// Stream tokens from Ollama. Each yielded item is one token/text chunk
    /// as it arrives; Ollama's NDJSON lines don't line up with HTTP chunk
    /// boundaries, so this buffers partial lines across chunks rather than
    /// assuming one JSON object per `bytes_stream` item.
    pub async fn stream(
        &self,
        system: &str,
        prompt: &str,
    ) -> Result<impl tokio_stream::Stream<Item = Result<String>>> {
        let request = GenerateRequest {
            model: self.model.clone(),
            prompt: prompt.to_string(),
            system: Some(system.to_string()),
            stream: true,
            format: None,
        };

        debug!("Starting stream request to Ollama");

        let response = self
            .client
            .post(format!("{}/api/generate", self.base_url))
            .json(&request)
            .send()
            .await?;

        // Forward tokens through a channel rather than a chained combinator
        // pipeline: the source is `bytes_stream()` items, which don't line up
        // with NDJSON line boundaries, and a single chunk can yield zero,
        // one, or several tokens - a plain task loop makes that fan-out (and
        // buffering partial lines across chunks) far more readable.
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<String>>();

        tokio::spawn(async move {
            let mut bytes_stream = response.bytes_stream();
            let mut line_buf = String::new();

            while let Some(chunk) = bytes_stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = tx.send(Err(e.into()));
                        return;
                    }
                };
                line_buf.push_str(&String::from_utf8_lossy(&bytes));

                while let Some(newline_pos) = line_buf.find('\n') {
                    let line = line_buf[..newline_pos].to_string();
                    line_buf.drain(..=newline_pos);
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<GenerateResponse>(&line) {
                        Ok(resp) => {
                            if !resp.response.is_empty() && tx.send(Ok(resp.response)).is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e.into()));
                            return;
                        }
                    }
                }
            }
        });

        Ok(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
    }

    /// Stream tokens from Ollama's `/api/chat` endpoint, which - unlike
    /// `/api/generate` - takes a full list of prior turns so the model can
    /// see the conversation instead of a single isolated prompt. `messages`
    /// should already include a leading system message if one is wanted.
    pub async fn stream_chat(
        &self,
        messages: Vec<ChatMessage>,
    ) -> Result<impl tokio_stream::Stream<Item = Result<ChatChunk>>> {
        let request = ChatRequest {
            model: self.model.clone(),
            messages,
            stream: true,
            think: true,
        };

        debug!("Starting chat stream request to Ollama");

        let response = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&request)
            .send()
            .await?;

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<ChatChunk>>();

        tokio::spawn(async move {
            let mut bytes_stream = response.bytes_stream();
            let mut line_buf = String::new();

            while let Some(chunk) = bytes_stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = tx.send(Err(e.into()));
                        return;
                    }
                };
                line_buf.push_str(&String::from_utf8_lossy(&bytes));

                while let Some(newline_pos) = line_buf.find('\n') {
                    let line = line_buf[..newline_pos].to_string();
                    line_buf.drain(..=newline_pos);
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<ChatResponse>(&line) {
                        Ok(resp) => {
                            let message = resp.message;
                            if let Some(thinking) = message.thinking {
                                if !thinking.is_empty()
                                    && tx.send(Ok(ChatChunk::Thinking(thinking))).is_err()
                                {
                                    return;
                                }
                            }
                            if !message.content.is_empty()
                                && tx.send(Ok(ChatChunk::Content(message.content))).is_err()
                            {
                                return;
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e.into()));
                            return;
                        }
                    }
                }
            }
        });

        Ok(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
    }

    /// Probe whether the *currently selected model* can actually serve a
    /// generation - not just whether the Ollama daemon is up (`health_check`
    /// only pings `/api/tags`, which succeeds even when the model can't
    /// actually answer: not pulled, not signed in for a cloud model, or a
    /// cloud model whose first call is still cold-starting).
    ///
    /// Bounded to roughly [`DIAGNOSE_TIMEOUT`] total. The probe asks for a
    /// single token with `think` off - a reasoning trace would only delay the
    /// first byte this is timing. Every returned [`OllamaDiagnosis`] variant
    /// maps to one concrete thing to tell the user.
    pub async fn diagnose(&self) -> OllamaDiagnosis {
        let model = self.model.clone();
        let cloud = is_cloud_model(&model);

        // 1. Is the daemon reachable at all?
        match self
            .client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => {
                return OllamaDiagnosis::ServerError {
                    status: r.status().as_u16(),
                    body: r.text().await.unwrap_or_default(),
                }
            }
            Err(e) => {
                return OllamaDiagnosis::DaemonUnreachable {
                    endpoint: self.base_url.clone(),
                    detail: e.to_string(),
                }
            }
        }

        // 2. For a *local* model, is it actually pulled? (Cloud models don't
        //    need pulling and may or may not appear in `/api/tags` depending
        //    on the Ollama version, so this check is local-only.)
        if !cloud {
            if let Ok(models) = self.list_models().await {
                let known = models
                    .iter()
                    .any(|m| m == &model || strip_model_tag(m) == strip_model_tag(&model));
                if !known {
                    return OllamaDiagnosis::ModelNotFound { model };
                }
            }
        }

        // 3. Can it generate? Ask for one token and time the first byte.
        let request = ChatRequest {
            model: model.clone(),
            messages: vec![ChatMessage::user("ping")],
            stream: true,
            think: false,
        };
        let started = Instant::now();
        let send = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&request)
            .send();
        let response = match tokio::time::timeout(DIAGNOSE_TIMEOUT, send).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                return OllamaDiagnosis::DaemonUnreachable {
                    endpoint: self.base_url.clone(),
                    detail: e.to_string(),
                }
            }
            Err(_) => return diagnose_stalled(cloud, &model, started),
        };

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            if cloud && (status == 401 || status == 403 || body_mentions_auth(&body)) {
                return OllamaDiagnosis::CloudSignInRequired { model };
            }
            return OllamaDiagnosis::ServerError { status, body };
        }

        // 200, but the body might still be a single JSON error line rather
        // than a token (some Ollama versions report a cloud-auth failure that
        // way), so inspect the first chunk before declaring success.
        let remaining = DIAGNOSE_TIMEOUT.saturating_sub(started.elapsed());
        let mut stream = response.bytes_stream();
        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                let text = String::from_utf8_lossy(&chunk);
                if cloud && text.contains("\"error\"") && body_mentions_auth(&text) {
                    OllamaDiagnosis::CloudSignInRequired { model }
                } else {
                    OllamaDiagnosis::Ok
                }
            }
            Ok(Some(Err(e))) => OllamaDiagnosis::DaemonUnreachable {
                endpoint: self.base_url.clone(),
                detail: e.to_string(),
            },
            Ok(None) | Err(_) => diagnose_stalled(cloud, &model, started),
        }
    }
}

/// Total wall-clock budget for [`OllamaClient::diagnose`] - long enough for a
/// cloud round-trip or a cold local model to produce its first token, short
/// enough that the UI's diagnostic modal never feels hung.
const DIAGNOSE_TIMEOUT: Duration = Duration::from_secs(20);

/// Outcome of [`OllamaClient::diagnose`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OllamaDiagnosis {
    /// Daemon answered and the selected model streamed at least one chunk.
    Ok,
    /// Could not reach the Ollama daemon at the configured endpoint.
    DaemonUnreachable { endpoint: String, detail: String },
    /// Daemon is up, but the selected (local) model isn't pulled.
    ModelNotFound { model: String },
    /// A `:cloud` model rejected with an *explicit* auth-shaped error
    /// (401/403, or an error body naming credentials / sign-in): the remedy
    /// is unambiguously `ollama signin`.
    CloudSignInRequired { model: String },
    /// A `:cloud` model that accepted the request but produced no token
    /// before the deadline. Ambiguous by nature: the first call to a cloud
    /// model routes through Ollama's hosted side and can take a while to
    /// warm up (a plain cold start, fixed by retrying), *or* the session
    /// isn't authenticated (fixed by `ollama signin`). We can't tell which
    /// from a timeout alone, so the message covers both.
    CloudUnresponsive { model: String, waited_secs: u64 },
    /// `/api/chat` returned an HTTP error status.
    ServerError { status: u16, body: String },
    /// Daemon up, *local* model present, request accepted, but no token
    /// arrived before the deadline - model stuck loading, too large, or wedged.
    ModelUnresponsive { model: String, waited_secs: u64 },
}

/// Whether a model name refers to an Ollama Cloud model (served from Ollama's
/// hosted infrastructure rather than pulled locally). These need `ollama
/// signin`, and their first call can also be slow to warm up on the hosted
/// side - either way `/api/chat` can sit with no response and no error, which
/// is the failure [`OllamaClient::diagnose`] exists to name.
pub fn is_cloud_model(model: &str) -> bool {
    let tag = model.rsplit(':').next().unwrap_or("");
    tag == "cloud" || tag.ends_with("-cloud") || model.contains("-cloud")
}

/// `"qwen3:4b"` -> `"qwen3"`. Used to match a selected model against
/// `/api/tags` when the tag differs but the family is present.
fn strip_model_tag(model: &str) -> &str {
    model.split(':').next().unwrap_or(model)
}

/// Heuristic: does this response body read like an authentication failure?
fn body_mentions_auth(body: &str) -> bool {
    let b = body.to_lowercase();
    b.contains("unauthorized")
        || b.contains("sign in")
        || b.contains("signin")
        || b.contains("sign-in")
        || b.contains("api key")
        || b.contains("apikey")
        || b.contains("authenticat")
        || b.contains("not logged in")
        || b.contains("ollama.com/settings/keys")
}

/// A probe that timed out or closed empty *without* an explicit error: for a
/// cloud model this is the ambiguous cold-start-or-not-signed-in case; for a
/// local one, the model is wedged/still loading.
fn diagnose_stalled(cloud: bool, model: &str, started: Instant) -> OllamaDiagnosis {
    let waited_secs = started.elapsed().as_secs();
    if cloud {
        OllamaDiagnosis::CloudUnresponsive {
            model: model.to_string(),
            waited_secs,
        }
    } else {
        OllamaDiagnosis::ModelUnresponsive {
            model: model.to_string(),
            waited_secs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::test;

    // Requires a live Ollama server on localhost:11434 with `qwen3.5:4b` pulled - not mocked yet
    // (see roadmap P0.4), so it can't run in CI. Run it locally with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    async fn test_ollama_client() {
        let client = OllamaClient::new("http://localhost:11434", "qwen3.5:4b");
        assert!(client.health_check().await);
    }

    #[test]
    async fn diagnose_reports_unreachable_daemon() {
        // Nothing is listening on this port - the probe must fail fast with
        // `DaemonUnreachable`, not hang or panic.
        let client = OllamaClient::new("http://127.0.0.1:1", "llama3.2:3b");
        assert!(matches!(
            client.diagnose().await,
            OllamaDiagnosis::DaemonUnreachable { .. }
        ));
    }

    #[test]
    async fn cloud_model_detection() {
        assert!(is_cloud_model("nemotron-3-ultra:cloud"));
        assert!(is_cloud_model("gpt-oss:120b-cloud"));
        assert!(is_cloud_model("deepseek-v3.1:671b-cloud"));
        assert!(!is_cloud_model("qwen3:4b"));
        assert!(!is_cloud_model("llama3.2:3b"));
        assert!(!is_cloud_model("mycloud-model:7b"));
    }
}
