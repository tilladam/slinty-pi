//! HTTP client for llama.cpp's router mode (`llama-server` launched without
//! `--model`, per pi's `LLAMA_BASE_URL` / `/login llama.cpp` wiring).
//!
//! Endpoints and JSON shapes verified against ggml-org/llama.cpp
//! `tools/server/README.md` (the "Using multiple models" section) as of
//! 2026-07. Deserialization is tolerant of unknown fields and status values,
//! matching pi-rpc's convention (see its module doc comment), since the
//! router API is still evolving upstream.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8080";

#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    #[error("router request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("router returned an error ({code}): {message}")]
    Api { code: u16, message: String },
    #[allow(dead_code)] // returned by the SSE stream, unused until item 5 wires it in
    #[error("malformed SSE stream: {0}")]
    Sse(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthState {
    Ready,
    Loading,
    Unreachable,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    #[allow(dead_code)] // not shown in the panel yet — item 5's model cards want it
    #[serde(default)]
    pub path: Option<String>,
    pub status: ModelStatus,
    #[allow(dead_code)] // not shown in the panel yet — item 5's model cards want it
    #[serde(default)]
    pub architecture: Option<Architecture>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Architecture {
    #[allow(dead_code)] // not shown in the panel yet — item 5's model cards want it
    #[serde(default)]
    pub input_modalities: Vec<String>,
    #[allow(dead_code)] // not shown in the panel yet — item 5's model cards want it
    #[serde(default)]
    pub output_modalities: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusValue {
    Loaded,
    Unloaded,
    Loading,
    Sleeping,
    Downloading,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelStatus {
    pub value: StatusValue,
    #[allow(dead_code)] // not shown in the panel yet — item 5's model cards want it
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub failed: bool,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub progress: Option<StatusProgress>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileProgress {
    pub done: u64,
    pub total: u64,
}

/// Progress for a `loading` model: `stages` lists the load phases in order
/// (e.g. text/spec/mmproj model), `current` is the phase in flight, `value`
/// is that phase's fractional progress. mmap may misreport this on some
/// platforms (upstream's note: pass `--no-mmap` for exact progress).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadingProgress {
    #[allow(dead_code)] // not shown in the panel yet — `current` covers the label for now
    #[serde(default)]
    pub stages: Vec<String>,
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub value: Option<f64>,
}

/// The same `progress` field shape differs by status: an object with
/// `stages`/`current`/`value` while loading, or a map of URL -> byte
/// counters while downloading. `LoadingProgress` denies unknown fields so
/// untagged deserialization falls through to the download map instead of
/// silently matching it (all its fields are optional).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum StatusProgress {
    Loading(LoadingProgress),
    Downloading(HashMap<String, FileProgress>),
}

pub struct LlamaRouter {
    client: reqwest::Client,
    base_url: String,
}

impl Default for LlamaRouter {
    fn default() -> Self {
        Self::new(DEFAULT_BASE_URL)
    }
}

impl LlamaRouter {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client builds"),
            base_url: base_url.into(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// `GET /health` — public, no API key check. 200 once ready, 503 while
    /// loading; any other outcome (connection refused, timeout, ...) means
    /// no router is listening at this URL.
    pub async fn health(&self) -> HealthState {
        match self
            .client
            .get(format!("{}/health", self.base_url))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => HealthState::Ready,
            Ok(resp) if resp.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE => {
                HealthState::Loading
            }
            _ => HealthState::Unreachable,
        }
    }

    /// `GET /models` (optionally `?reload=1` to rescan the model sources).
    pub async fn list_models(&self, reload: bool) -> Result<Vec<ModelEntry>, RouterError> {
        let mut url = format!("{}/models", self.base_url);
        if reload {
            url.push_str("?reload=1");
        }
        let resp = check_status(self.client.get(url).send().await?).await?;
        let body: ModelsResponse = resp.json().await?;
        Ok(body.data)
    }

    /// `POST /models/load`.
    pub async fn load_model(&self, model: &str) -> Result<(), RouterError> {
        self.post_model_action("/models/load", model).await
    }

    /// `POST /models/unload` (also cancels an in-flight download for `model`).
    pub async fn unload_model(&self, model: &str) -> Result<(), RouterError> {
        self.post_model_action("/models/unload", model).await
    }

    /// `POST /models` — starts a non-blocking download; progress is polled
    /// from [`Self::list_models`] (see `poll_router_until_idle` in
    /// `backend.rs`), not [`Self::subscribe_events`].
    pub async fn download_model(&self, model: &str) -> Result<(), RouterError> {
        self.post_model_action("/models", model).await
    }

    /// `DELETE /models?model=...` — cache-only; fails for preset-defined models.
    #[allow(dead_code)] // item 5's HF-search/download flow
    pub async fn delete_model(&self, model: &str) -> Result<(), RouterError> {
        let resp = self
            .client
            .delete(format!("{}/models", self.base_url))
            .query(&[("model", model)])
            .send()
            .await?;
        check_status(resp).await?;
        Ok(())
    }

    async fn post_model_action(&self, path: &str, model: &str) -> Result<(), RouterError> {
        let resp = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .json(&serde_json::json!({ "model": model }))
            .send()
            .await?;
        check_status(resp).await?;
        Ok(())
    }

    /// Open `GET /models/sse` for real-time load/download progress. Call
    /// [`SseReader::next_event`] in a loop; it returns `Ok(None)` when the
    /// router closes the stream.
    ///
    /// Unused for now: polling `GET /models` (whose `status.progress` field
    /// already carries the same data) is enough for the models panel's
    /// progress display — see `router_model_status_label` in `backend.rs`.
    /// A future live-updating panel could switch to this instead.
    #[allow(dead_code)]
    pub async fn subscribe_events(&self) -> Result<SseReader, RouterError> {
        let resp = self
            .client
            .get(format!("{}/models/sse", self.base_url))
            .header("Accept", "text/event-stream")
            .send()
            .await?;
        let resp = check_status(resp).await?;
        Ok(SseReader {
            resp: Some(resp),
            buf: Vec::new(),
        })
    }
}

async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, RouterError> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    let code = resp.status().as_u16();
    let message = match resp.json::<ApiErrorBody>().await {
        Ok(body) => body.error.message,
        Err(_) => format!("HTTP {code}"),
    };
    Err(RouterError::Api { code, message })
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    error: ApiErrorInner,
}

#[derive(Debug, Deserialize)]
struct ApiErrorInner {
    message: String,
}

/// One parsed `/models/sse` event. Event kinds not recognized here surface as
/// `Unknown` instead of failing the stream, so router additions don't break
/// the client (see module doc comment).
///
/// Unused for now — see [`LlamaRouter::subscribe_events`]'s doc comment.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum RouterEvent {
    ModelStatus {
        model: String,
        status: SseStatusValue,
        progress: Option<LoadingProgress>,
        /// Only present on the first `loaded` event after a cold load.
        info: Option<Value>,
    },
    DownloadProgress {
        model: String,
        files: HashMap<String, FileProgress>,
    },
    ModelRemove {
        model: String,
    },
    /// `model == "*"`: the router wants a full `/models` re-fetch.
    ModelsReload,
    Unknown {
        model: String,
        event: String,
    },
}

#[allow(dead_code)] // see LlamaRouter::subscribe_events's doc comment
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SseStatusValue {
    Loading,
    Loaded,
    Sleeping,
    Unloaded,
    Downloading,
    #[serde(other)]
    Unknown,
}

#[allow(dead_code)] // see LlamaRouter::subscribe_events's doc comment
#[derive(Debug, Clone, Deserialize)]
struct SseEnvelope {
    model: String,
    event: String,
    #[serde(default)]
    data: Option<Value>,
}

#[allow(dead_code)] // see LlamaRouter::subscribe_events's doc comment
#[derive(Debug, Clone, Deserialize)]
struct SseModelStatusData {
    status: SseStatusValue,
    #[serde(default)]
    progress: Option<LoadingProgress>,
    #[serde(default)]
    info: Option<Value>,
}

#[allow(dead_code)] // see LlamaRouter::subscribe_events's doc comment
impl RouterEvent {
    fn from_envelope(env: SseEnvelope) -> Result<Self, RouterError> {
        match env.event.as_str() {
            "model_status" => {
                let data = env
                    .data
                    .ok_or_else(|| RouterError::Sse("model_status event missing data".into()))?;
                let data: SseModelStatusData =
                    serde_json::from_value(data).map_err(|e| RouterError::Sse(e.to_string()))?;
                Ok(RouterEvent::ModelStatus {
                    model: env.model,
                    status: data.status,
                    progress: data.progress,
                    info: data.info,
                })
            }
            "download_progress" => {
                let data = env.data.ok_or_else(|| {
                    RouterError::Sse("download_progress event missing data".into())
                })?;
                let files: HashMap<String, FileProgress> =
                    serde_json::from_value(data).map_err(|e| RouterError::Sse(e.to_string()))?;
                Ok(RouterEvent::DownloadProgress {
                    model: env.model,
                    files,
                })
            }
            "model_remove" => Ok(RouterEvent::ModelRemove { model: env.model }),
            "models_reload" => Ok(RouterEvent::ModelsReload),
            other => Ok(RouterEvent::Unknown {
                model: env.model,
                event: other.to_string(),
            }),
        }
    }
}

#[allow(dead_code)] // see LlamaRouter::subscribe_events's doc comment
fn find_double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}

/// Extracts the JSON envelope from one SSE block (the `data:` line(s)
/// between blank-line separators). Returns `Ok(None)` for blocks that carry
/// no `data:` line (e.g. a bare comment/keepalive).
#[allow(dead_code)] // see LlamaRouter::subscribe_events's doc comment
fn parse_sse_block(block: &[u8]) -> Result<Option<SseEnvelope>, RouterError> {
    let text = String::from_utf8_lossy(block);
    let payload: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|rest| rest.strip_prefix(' ').unwrap_or(rest))
        .collect();
    if payload.is_empty() {
        return Ok(None);
    }
    let joined = payload.join("\n");
    serde_json::from_str(&joined)
        .map(Some)
        .map_err(|e| RouterError::Sse(format!("{e}: {joined}")))
}

/// Streaming reader over `/models/sse`, one [`RouterEvent`] at a time.
#[allow(dead_code)] // see LlamaRouter::subscribe_events's doc comment
pub struct SseReader {
    resp: Option<reqwest::Response>,
    buf: Vec<u8>,
}

#[allow(dead_code)] // see LlamaRouter::subscribe_events's doc comment
impl SseReader {
    pub async fn next_event(&mut self) -> Result<Option<RouterEvent>, RouterError> {
        loop {
            if let Some(pos) = find_double_newline(&self.buf) {
                let block: Vec<u8> = self.buf.drain(..pos).collect();
                self.buf.drain(..2); // the "\n\n" separator itself
                if let Some(envelope) = parse_sse_block(&block)? {
                    return Ok(Some(RouterEvent::from_envelope(envelope)?));
                }
                continue;
            }
            let Some(resp) = self.resp.as_mut() else {
                return Ok(None);
            };
            match resp.chunk().await? {
                // Drop `\r` on ingestion so `\r\n\r\n` (a valid SSE block
                // separator) still hits the `\n\n` scan below instead of
                // buffering forever waiting for a separator that never
                // matches byte-for-byte.
                Some(bytes) => self
                    .buf
                    .extend(bytes.iter().copied().filter(|&b| b != b'\r')),
                None => {
                    self.resp = None;
                    let tail = std::mem::take(&mut self.buf);
                    if let Some(envelope) = parse_sse_block(&tail)? {
                        return Ok(Some(RouterEvent::from_envelope(envelope)?));
                    }
                    return Ok(None);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_model_list_with_mixed_statuses() {
        let body = r#"{
            "data": [
                {
                    "id": "ggml-org/gemma-3-4b-it-GGUF:Q4_K_M",
                    "path": "/cache/gemma.gguf",
                    "status": { "value": "loaded", "args": ["llama-server", "-ctx", "4096"] },
                    "architecture": { "input_modalities": ["text", "image"], "output_modalities": ["text"] }
                },
                {
                    "id": "some/other-model",
                    "status": { "value": "unloaded" }
                },
                {
                    "id": "some/failed-model",
                    "status": { "value": "unloaded", "args": ["llama-server"], "failed": true, "exit_code": 1 }
                },
                {
                    "id": "some/downloading-model",
                    "status": {
                        "value": "downloading",
                        "progress": { "https://x/model.gguf": { "done": 195963406, "total": 219307424 } }
                    }
                }
            ]
        }"#;
        let parsed: ModelsResponse = serde_json::from_str(body).expect("parses");
        assert_eq!(parsed.data.len(), 4);

        let loaded = &parsed.data[0];
        assert_eq!(loaded.status.value, StatusValue::Loaded);
        assert_eq!(
            loaded.architecture.as_ref().unwrap().input_modalities,
            vec!["text", "image"]
        );

        let failed = &parsed.data[2];
        assert!(failed.status.failed);
        assert_eq!(failed.status.exit_code, Some(1));

        let downloading = &parsed.data[3];
        match downloading.status.progress.as_ref().unwrap() {
            StatusProgress::Downloading(files) => {
                let f = &files["https://x/model.gguf"];
                assert_eq!(f.done, 195963406);
                assert_eq!(f.total, 219307424);
            }
            StatusProgress::Loading(_) => panic!("expected a download progress map"),
        }
    }

    #[test]
    fn parses_loading_progress_with_stages() {
        let status: ModelStatus = serde_json::from_str(
            r#"{
                "value": "loading",
                "progress": { "stages": ["text_model", "mmproj_model"], "current": "text_model", "value": 0.5 }
            }"#,
        )
        .expect("parses");
        match status.progress.unwrap() {
            StatusProgress::Loading(p) => {
                assert_eq!(p.stages, vec!["text_model", "mmproj_model"]);
                assert_eq!(p.current.as_deref(), Some("text_model"));
                assert_eq!(p.value, Some(0.5));
            }
            StatusProgress::Downloading(_) => panic!("expected loading progress"),
        }
    }

    #[test]
    fn unknown_status_value_falls_back() {
        let status: ModelStatus =
            serde_json::from_str(r#"{ "value": "hibernating" }"#).expect("parses");
        assert_eq!(status.value, StatusValue::Unknown);
    }

    #[test]
    fn parses_sse_model_status_event() {
        let env: SseEnvelope = serde_json::from_str(
            r#"{ "model": "m", "event": "model_status", "data": { "status": "loading" } }"#,
        )
        .expect("parses");
        let event = RouterEvent::from_envelope(env).expect("converts");
        match event {
            RouterEvent::ModelStatus {
                model,
                status,
                progress,
                info,
            } => {
                assert_eq!(model, "m");
                assert_eq!(status, SseStatusValue::Loading);
                assert!(progress.is_none());
                assert!(info.is_none());
            }
            _ => panic!("expected ModelStatus"),
        }
    }

    #[test]
    fn parses_sse_download_progress_event() {
        let env: SseEnvelope = serde_json::from_str(
            r#"{
                "model": "m",
                "event": "download_progress",
                "data": { "https://x/model.gguf": { "done": 10, "total": 100 } }
            }"#,
        )
        .expect("parses");
        let event = RouterEvent::from_envelope(env).expect("converts");
        match event {
            RouterEvent::DownloadProgress { model, files } => {
                assert_eq!(model, "m");
                assert_eq!(files["https://x/model.gguf"].done, 10);
            }
            _ => panic!("expected DownloadProgress"),
        }
    }

    #[test]
    fn parses_sse_events_without_data() {
        let remove: SseEnvelope =
            serde_json::from_str(r#"{ "model": "m", "event": "model_remove" }"#).expect("parses");
        assert!(matches!(
            RouterEvent::from_envelope(remove).expect("converts"),
            RouterEvent::ModelRemove { model } if model == "m"
        ));

        let reload: SseEnvelope =
            serde_json::from_str(r#"{ "model": "*", "event": "models_reload" }"#).expect("parses");
        assert!(matches!(
            RouterEvent::from_envelope(reload).expect("converts"),
            RouterEvent::ModelsReload
        ));
    }

    #[test]
    fn unknown_sse_event_kind_falls_back() {
        let env: SseEnvelope =
            serde_json::from_str(r#"{ "model": "m", "event": "something_new" }"#).expect("parses");
        assert!(matches!(
            RouterEvent::from_envelope(env).expect("converts"),
            RouterEvent::Unknown { event, .. } if event == "something_new"
        ));
    }

    #[test]
    fn sse_block_parser_splits_multiple_frames() {
        let stream = b"data: {\"model\":\"a\",\"event\":\"model_remove\"}\n\n\
                        : keepalive\n\n\
                        data: {\"model\":\"b\",\"event\":\"models_reload\"}\n\n";

        let mut remaining = &stream[..];
        let mut events = Vec::new();
        while let Some(pos) = find_double_newline(remaining) {
            let block = &remaining[..pos];
            if let Some(env) = parse_sse_block(block).expect("parses") {
                events.push(env);
            }
            remaining = &remaining[pos + 2..];
        }
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].model, "a");
        assert_eq!(events[1].model, "b");
    }
}
