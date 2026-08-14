//! Swift-facing mirrors of `pi_local::panel`'s row/summary data — the
//! payloads `LocalModelIndex`'s refresh/search methods return. Plain UniFFI
//! `Record`s with `From` conversions, same pattern as `row.rs`'s
//! `RowRecord: From<pi_render::RowSpec>`.

/// Mirrors `pi_local::panel::RapidMlxModelState` — drives which single
/// action a cached-model row offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RapidMlxModelStateRecord {
    /// Known to pi and currently served → offer Stop.
    KnownServed,
    /// Known to pi, nothing serving it → offer Serve.
    KnownIdle,
    /// Not in pi's model list → offer Register.
    Unknown,
}

impl From<pi_local::panel::RapidMlxModelState> for RapidMlxModelStateRecord {
    fn from(state: pi_local::panel::RapidMlxModelState) -> Self {
        match state {
            pi_local::panel::RapidMlxModelState::KnownServed => Self::KnownServed,
            pi_local::panel::RapidMlxModelState::KnownIdle => Self::KnownIdle,
            pi_local::panel::RapidMlxModelState::Unknown => Self::Unknown,
        }
    }
}

#[derive(uniffi::Record)]
pub struct CachedModelRecord {
    pub alias: String,
    pub hf_repo: String,
    pub size: String,
    pub fit_label: String,
    pub state: RapidMlxModelStateRecord,
}

impl From<pi_local::panel::CachedModelRow> for CachedModelRecord {
    fn from(row: pi_local::panel::CachedModelRow) -> Self {
        Self {
            alias: row.alias,
            hf_repo: row.hf_repo,
            size: row.size,
            fit_label: row.fit_label,
            state: row.state.into(),
        }
    }
}

/// The running rapid-mlx server, if any — see
/// `pi_local::panel::RunningInfo`.
#[derive(uniffi::Record)]
pub struct RunningServerRecord {
    pub summary: String,
    /// `false` warrants a warning in the UI: up, but pi can't route to it.
    pub known_to_pi: bool,
    /// Only a server this app spawned can be stopped from here.
    pub managed: bool,
}

impl From<pi_local::panel::RunningInfo> for RunningServerRecord {
    fn from(info: pi_local::panel::RunningInfo) -> Self {
        Self {
            summary: info.summary,
            known_to_pi: info.known_to_pi,
            managed: info.managed,
        }
    }
}

#[derive(uniffi::Record)]
pub struct RapidMlxPanelRecord {
    pub version: Option<String>,
    pub running: Option<RunningServerRecord>,
    pub cached: Vec<CachedModelRecord>,
    pub catalog_count: u32,
}

impl From<pi_local::panel::RapidMlxPanelData> for RapidMlxPanelRecord {
    fn from(data: pi_local::panel::RapidMlxPanelData) -> Self {
        Self {
            version: data.version,
            running: data.running.map(RunningServerRecord::from),
            cached: data
                .cached
                .into_iter()
                .map(CachedModelRecord::from)
                .collect(),
            catalog_count: data.catalog_count as u32,
        }
    }
}

#[derive(uniffi::Record)]
pub struct RouterModelRecord {
    pub id: String,
    pub status_label: String,
    pub loaded: bool,
    pub busy: bool,
}

impl From<(String, String, bool, bool)> for RouterModelRecord {
    fn from((id, status_label, loaded, busy): (String, String, bool, bool)) -> Self {
        Self {
            id,
            status_label,
            loaded,
            busy,
        }
    }
}

#[derive(uniffi::Record)]
pub struct RouterPanelRecord {
    pub status_label: String,
    pub base_url: String,
    pub models: Vec<RouterModelRecord>,
}

impl From<pi_local::panel::RouterPanelData> for RouterPanelRecord {
    fn from(data: pi_local::panel::RouterPanelData) -> Self {
        Self {
            status_label: data.status_label,
            base_url: data.base_url,
            models: data
                .models
                .into_iter()
                .map(RouterModelRecord::from)
                .collect(),
        }
    }
}

#[derive(uniffi::Record)]
pub struct HfResultRecord {
    pub id: String,
    pub gated: bool,
    pub downloads: i32,
    pub quants: Vec<String>,
}

impl From<(String, bool, i32, Vec<String>)> for HfResultRecord {
    fn from((id, gated, downloads, quants): (String, bool, i32, Vec<String>)) -> Self {
        Self {
            id,
            gated,
            downloads,
            quants,
        }
    }
}

#[derive(uniffi::Record)]
pub struct OllamaPanelRecord {
    pub detected: bool,
    pub summary: String,
    pub model_count: i32,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum LocalModelError {
    #[error("{0}")]
    Action(String),
}

const HF_SEARCH_LIMIT: u32 = 20;

/// Stateless local-model browsing/management — no live `pi` child needed
/// (mirrors `SessionIndex`, which also never touches a `PiClient`), so the
/// Models panel can render/act before/independent of `PiSession` spawning.
/// Every method is a thin call into `pi_local::{router,hf,ollama,auth_json,
/// models_json,system_fit}`/`pi_local::panel`, mirroring `pi_core::backend`'s
/// reference handlers' sequence minus the `Transcript`/`UiSink` push
/// (replaced by a return value) and minus the "nudge pi's own model picker"
/// step those handlers do afterward — no picker exists in Swift yet (SW4's
/// explicit scope cut, see the crate/plan doc comments).
#[derive(uniffi::Object, Default)]
pub struct LocalModelIndex;

#[uniffi::export(async_runtime = "tokio")]
impl LocalModelIndex {
    #[uniffi::constructor]
    pub fn new() -> Self {
        crate::ensure_logging_initialized();
        Self
    }

    pub async fn refresh_router_panel(&self) -> RouterPanelRecord {
        let router = pi_local::router::LlamaRouter::default();
        RouterPanelRecord::from(pi_local::panel::fetch_router_state(&router).await)
    }

    pub async fn refresh_ollama_panel(&self) -> OllamaPanelRecord {
        let models = pi_local::ollama::OllamaProbe::default().list_models().await;
        let (detected, summary, model_count) = pi_local::panel::format_ollama_panel(models);
        OllamaPanelRecord {
            detected,
            summary,
            model_count,
        }
    }

    /// (Re)loads auth.json's entry list. Unreadable/malformed surfaces as a
    /// single pseudo-entry rather than an empty list pretending there are no
    /// credentials — matches `pi_core::backend::refresh_auth_entries`.
    pub async fn refresh_auth_entries(&self) -> Vec<String> {
        match pi_local::auth_json::default_path() {
            Some(path) => auth_entries_at(&path),
            None => vec!["auth.json: no home directory".to_string()],
        }
    }

    pub async fn search_hf_models(
        &self,
        query: String,
    ) -> Result<Vec<HfResultRecord>, LocalModelError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        pi_local::hf::HfSearch::default()
            .search_gguf(&query, HF_SEARCH_LIMIT)
            .await
            .map(|models| {
                pi_local::panel::format_hf_results(models)
                    .into_iter()
                    .map(HfResultRecord::from)
                    .collect()
            })
            .map_err(|e| LocalModelError::Action(format!("Hugging Face search failed: {e}")))
    }

    /// Fires `POST /models/load` and returns immediately — deliberately not
    /// a port of `pi_core::backend::poll_router_until_idle`'s blocking
    /// 500ms/120s loop (an awkward fit for a UniFFI async call and gives no
    /// way to show live progress mid-poll). Swift owns the polling instead,
    /// calling `refresh_router_panel` on its own bounded cadence.
    pub async fn start_load_router_model(&self, id: String) -> Result<(), LocalModelError> {
        pi_local::router::LlamaRouter::default()
            .load_model(&id)
            .await
            .map_err(|e| LocalModelError::Action(format!("router: failed to load {id}: {e}")))
    }

    pub async fn start_unload_router_model(&self, id: String) -> Result<(), LocalModelError> {
        pi_local::router::LlamaRouter::default()
            .unload_model(&id)
            .await
            .map_err(|e| LocalModelError::Action(format!("router: failed to unload {id}: {e}")))
    }

    /// `POST /models` (download-only, doesn't load) — same one-shot-then-
    /// Swift-polls shape as `start_load_router_model`.
    pub async fn start_download_router_model(&self, model: String) -> Result<(), LocalModelError> {
        pi_local::router::LlamaRouter::default()
            .download_model(&model)
            .await
            .map_err(|e| {
                LocalModelError::Action(format!("router: failed to start download of {model}: {e}"))
            })
    }

    /// Writes every currently-detected Ollama model into `~/.pi/agent/
    /// models.json` under the canonical `ollama` preset. Refuses to touch a
    /// `models.json` it can't parse rather than guessing, matching
    /// `pi_core::backend::add_ollama_to_pi`.
    pub async fn add_ollama_to_pi(&self) -> Result<(), LocalModelError> {
        let Some(models) = pi_local::ollama::OllamaProbe::default().list_models().await else {
            return Err(LocalModelError::Action(
                "Ollama: no longer detected — nothing to add".to_string(),
            ));
        };
        if models.is_empty() {
            return Err(LocalModelError::Action(
                "Ollama: no models pulled yet — nothing to add".to_string(),
            ));
        }
        let Some(path) = pi_local::models_json::default_path() else {
            return Err(LocalModelError::Action(
                "could not resolve $HOME to locate models.json".to_string(),
            ));
        };
        let ids: Vec<String> = models.into_iter().map(|m| m.name).collect();
        add_ollama_ids_to_pi_at(&path, &ids)
    }

    /// Writes one api_key entry into auth.json (load -> edit -> atomic 0600
    /// write). Errors mention the provider only — never the key, matching
    /// `pi_core::backend::save_api_key`'s redaction contract.
    pub async fn save_api_key(&self, provider: String, key: String) -> Result<(), LocalModelError> {
        let Some(path) = pi_local::auth_json::default_path() else {
            return Err(LocalModelError::Action(
                "auth.json: no home directory".to_string(),
            ));
        };
        save_api_key_at(&path, &provider, &key)
    }
}

/// Split out from `refresh_auth_entries` purely so tests can point it at a
/// fixture path instead of the real `~/.pi/agent/auth.json` — same pattern
/// as `session_index.rs`'s `sessions_at(root: &Path, ...)`.
fn auth_entries_at(path: &std::path::Path) -> Vec<String> {
    match pi_local::auth_json::AuthJson::load_or_empty(path) {
        Ok(doc) => pi_local::panel::format_auth_entries(&doc.entries()),
        Err(e) => vec![format!("auth.json unreadable: {e}")],
    }
}

/// Split out from `save_api_key` for the same reason as `auth_entries_at`.
fn save_api_key_at(
    path: &std::path::Path,
    provider: &str,
    key: &str,
) -> Result<(), LocalModelError> {
    let mut doc = pi_local::auth_json::AuthJson::load_or_empty(path)
        .map_err(|e| LocalModelError::Action(format!("auth.json: {e}")))?;
    doc.set_api_key(provider, key)
        .map_err(|e| LocalModelError::Action(format!("auth.json: {e}")))?;
    doc.write(path)
        .map_err(|e| LocalModelError::Action(format!("auth.json: {e}")))
}

/// Split out from `add_ollama_to_pi` for the same reason as
/// `auth_entries_at` — takes already-known model ids so the models.json
/// read/replace/write logic is testable without a live Ollama.
fn add_ollama_ids_to_pi_at(path: &std::path::Path, ids: &[String]) -> Result<(), LocalModelError> {
    let mut doc = if path.exists() {
        pi_local::models_json::ModelsJson::load(path).map_err(|e| {
            LocalModelError::Action(format!(
                "{e} — refusing to overwrite a models.json I can't parse"
            ))
        })?
    } else {
        pi_local::models_json::ModelsJson::empty()
    };
    doc.set_provider("ollama", pi_local::ollama::provider_preset(ids));
    doc.write(path)
        .map_err(|e| LocalModelError::Action(format!("could not write models.json: {e}")))
}

/// Registers a rapid-mlx `alias` in `models.json`, returning the provider
/// key it landed under (which `set_model` must then be called with).
///
/// Path-parameterized for the same testability reason as
/// `add_ollama_ids_to_pi_at`. Merges rather than replaces — see
/// `pi_local::rapid_mlx::provider_preset`.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapid_mlx_panel_conversion_preserves_every_field() {
        let data = pi_local::panel::RapidMlxPanelData {
            version: Some("rapid-mlx 0.11.0".to_string()),
            running: Some(pi_local::panel::RunningInfo {
                summary: "model running on :8000".to_string(),
                known_to_pi: false,
                managed: true,
            }),
            cached: vec![pi_local::panel::CachedModelRow {
                alias: "alias".to_string(),
                hf_repo: "owner/repo".to_string(),
                size: "5.7 GiB".to_string(),
                fit_label: "Fits".to_string(),
                state: pi_local::panel::RapidMlxModelState::KnownIdle,
            }],
            catalog_count: 42,
        };
        let record = RapidMlxPanelRecord::from(data);
        assert_eq!(record.version.as_deref(), Some("rapid-mlx 0.11.0"));
        assert_eq!(record.catalog_count, 42);
        let running = record.running.expect("running server preserved");
        assert_eq!(running.summary, "model running on :8000");
        assert!(!running.known_to_pi);
        assert!(running.managed);
        assert_eq!(record.cached.len(), 1);
        assert_eq!(record.cached[0].alias, "alias");
        assert_eq!(record.cached[0].fit_label, "Fits");
        assert_eq!(record.cached[0].state, RapidMlxModelStateRecord::KnownIdle);
    }

    #[test]
    fn router_panel_conversion_preserves_every_field() {
        let data = pi_local::panel::RouterPanelData {
            status_label: "ready".to_string(),
            base_url: "http://127.0.0.1:8080".to_string(),
            models: vec![("id".to_string(), "loaded".to_string(), true, false)],
        };
        let record = RouterPanelRecord::from(data);
        assert_eq!(record.status_label, "ready");
        assert_eq!(record.models.len(), 1);
        assert_eq!(record.models[0].id, "id");
        assert!(record.models[0].loaded);
        assert!(!record.models[0].busy);
    }

    #[test]
    fn hf_result_conversion_preserves_every_field() {
        let record = HfResultRecord::from((
            "owner/repo".to_string(),
            true,
            100,
            vec!["Q4_K_M".to_string()],
        ));
        assert_eq!(record.id, "owner/repo");
        assert!(record.gated);
        assert_eq!(record.downloads, 100);
        assert_eq!(record.quants, vec!["Q4_K_M".to_string()]);
    }

    #[test]
    fn auth_entries_at_missing_file_is_empty_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("auth.json");
        assert_eq!(auth_entries_at(&path), Vec::<String>::new());
    }

    #[test]
    fn save_api_key_at_then_auth_entries_at_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("auth.json");
        save_api_key_at(&path, "anthropic", "sk-ant-super-secret").unwrap();
        // The panel-facing labels never carry key material, even though the
        // underlying auth.json (a 0600 credential store) legitimately does.
        let entries = auth_entries_at(&path);
        assert_eq!(entries, vec!["anthropic · api key".to_string()]);
        for entry in &entries {
            assert!(!entry.contains("super-secret"), "{entry}");
        }
    }

    #[test]
    fn add_ollama_ids_to_pi_at_creates_a_fresh_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("models.json");
        add_ollama_ids_to_pi_at(&path, &["llama3.1:8b".to_string()]).unwrap();
        let doc = pi_local::models_json::ModelsJson::load(&path).unwrap();
        let raw = String::from_utf8(doc.to_bytes()).unwrap();
        assert!(raw.contains("llama3.1:8b"));
        assert!(raw.contains("ollama"));
    }

    #[test]
    fn add_ollama_ids_to_pi_at_refuses_to_overwrite_unparseable_json() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("models.json");
        std::fs::write(&path, b"not json").unwrap();
        let err = add_ollama_ids_to_pi_at(&path, &["llama3.1:8b".to_string()]).unwrap_err();
        let LocalModelError::Action(message) = err;
        assert!(message.contains("refusing to overwrite"), "{message}");
    }
}
