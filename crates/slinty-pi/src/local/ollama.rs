//! Client for Ollama's native API — detection and installed-model listing,
//! feeding the models panel's "Ollama" section. Shape verified against
//! ollama/docs/api.md's `GET /api/tags` example; no live Ollama server was
//! running on this dev machine to test against (see the module comment on
//! `demo_ollama_models` in `backend.rs`).

use std::time::Duration;

use serde::Deserialize;

pub const DEFAULT_BASE_URL: &str = "http://localhost:11434";

#[derive(Debug, Clone, Deserialize)]
pub struct OllamaModel {
    /// `"<model>:<tag>"`, e.g. `"llama3.1:8b"` — this is exactly the `id`
    /// pi's `models.json` expects for an Ollama model entry (see
    /// `docs/models.md`'s minimal example).
    pub name: String,
    // Not shown in the panel yet — the current section is a one-click "add
    // all to pi" affordance (just names/ids), not a browsable per-model
    // detail list. Kept for a future richer view.
    #[allow(dead_code)]
    #[serde(default)]
    pub size: u64,
    #[allow(dead_code)]
    #[serde(default)]
    pub details: Option<OllamaModelDetails>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OllamaModelDetails {
    #[allow(dead_code)]
    #[serde(default)]
    pub parameter_size: String,
    #[allow(dead_code)]
    #[serde(default)]
    pub quantization_level: String,
}

#[derive(Debug, Clone, Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<OllamaModel>,
}

pub struct OllamaProbe {
    client: reqwest::Client,
    base_url: String,
}

impl Default for OllamaProbe {
    fn default() -> Self {
        Self::new(DEFAULT_BASE_URL)
    }
}

impl OllamaProbe {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("reqwest client builds"),
            base_url: base_url.into(),
        }
    }

    /// `GET /api/tags`. `None` covers every "no Ollama here" case alike (not
    /// installed, not running, a connection refused, an unexpected
    /// response) — the panel only needs detected-vs-not, not why.
    pub async fn list_models(&self) -> Option<Vec<OllamaModel>> {
        let resp = self
            .client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let body: TagsResponse = resp.json().await.ok()?;
        Some(body.models)
    }
}

/// The canonical Ollama provider preset from pi's own `docs/models.md`
/// "Minimal Example" — `baseUrl`/`api`/dummy `apiKey` verbatim, plus the
/// `compat` flags that same doc recommends specifically for Ollama (it
/// doesn't understand the `developer` role or `reasoning_effort`) — with
/// `model_ids` filled in from a live [`OllamaProbe::list_models`] call.
/// `contextWindow`/`maxTokens` are deliberately omitted: the doc states only
/// `id` is required for local models, and pi falls back to documented
/// defaults (128000 / 16384) for the rest.
pub fn provider_preset(model_ids: &[String]) -> serde_json::Value {
    serde_json::json!({
        "baseUrl": format!("{DEFAULT_BASE_URL}/v1"),
        "api": "openai-completions",
        "apiKey": "ollama",
        "compat": {
            "supportsDeveloperRole": false,
            "supportsReasoningEffort": false,
        },
        "models": model_ids
            .iter()
            .map(|id| serde_json::json!({ "id": id }))
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_preset_matches_pi_docs_minimal_example_shape() {
        let preset = provider_preset(&["llama3.1:8b".to_string(), "qwen2.5-coder:7b".to_string()]);
        assert_eq!(preset["baseUrl"], "http://localhost:11434/v1");
        assert_eq!(preset["api"], "openai-completions");
        assert_eq!(preset["apiKey"], "ollama");
        assert_eq!(preset["compat"]["supportsDeveloperRole"], false);
        assert_eq!(preset["models"][0]["id"], "llama3.1:8b");
        assert_eq!(preset["models"][1]["id"], "qwen2.5-coder:7b");
        // Only `id` per model — no contextWindow/maxTokens, per the doc's
        // "only `id` is required" note.
        assert_eq!(preset["models"][0].as_object().unwrap().len(), 1);
    }

    // ollama/docs/api.md's documented GET /api/tags example, verbatim.
    const TAGS_FIXTURE: &str = r#"{
        "models": [
            {
                "name": "deepseek-r1:latest",
                "model": "deepseek-r1:latest",
                "modified_at": "2025-05-10T08:06:48.639712648-07:00",
                "size": 4683075271,
                "digest": "0a8c266910232fd3291e71e5ba1e058cc5af9d411192cf88b6d30e92b6e73163",
                "details": {
                    "parent_model": "",
                    "format": "gguf",
                    "family": "qwen2",
                    "families": ["qwen2"],
                    "parameter_size": "7.6B",
                    "quantization_level": "Q4_K_M"
                }
            }
        ]
    }"#;

    #[test]
    fn parses_the_documented_tags_response() {
        let resp: TagsResponse = serde_json::from_str(TAGS_FIXTURE).expect("parses");
        assert_eq!(resp.models.len(), 1);
        assert_eq!(resp.models[0].name, "deepseek-r1:latest");
        assert_eq!(resp.models[0].size, 4683075271);
        assert_eq!(
            resp.models[0].details.as_ref().unwrap().parameter_size,
            "7.6B"
        );
    }

    #[test]
    fn missing_models_key_yields_an_empty_list_not_an_error() {
        let resp: TagsResponse = serde_json::from_str("{}").expect("parses");
        assert!(resp.models.is_empty());
    }
}
