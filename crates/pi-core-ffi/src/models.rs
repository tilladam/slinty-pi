//! Composer model picker + status-bar server-dot health indicator (SW6).
//! Ports (not shares — this crate doesn't depend on `pi-core`, matching its
//! established posture) `pi_core::backend`'s `ModelEntry`/`refresh_models`/
//! `model_label`/`is_local_base_url`/`probe_tcp`/`compute_server_dot`/
//! `classify_rapid_mlx_dot`.

use std::time::Duration;

use pi_rpc::PiClient;

/// Swift-facing picker entry: `label` is a fully-formatted display string
/// (see `model_label`), `is_current` flags whichever entry matches pi's
/// `GetState` response.
#[derive(Clone, uniffi::Record)]
pub struct ModelRecord {
    pub provider: String,
    pub id: String,
    pub label: String,
    pub is_current: bool,
}

/// Status-bar server-dot state — mirrors `pi_core::backend`'s
/// `SERVER_DOT_*` constants, typed for a nicer Swift `switch` instead of a
/// raw `Int`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, uniffi::Enum)]
pub enum ServerDotState {
    /// Hidden unless the current model is served from this machine.
    Hidden,
    Ok,
    Down,
    /// The server answers, but it's serving a *different* model than pi's
    /// current one — rapid-mlx 404s every completion in that state, so a
    /// plain "reachable" check would lie.
    Mismatch,
}

/// Internal-only (not exported to Swift) — everything `compute_server_dot`
/// needs to know about pi's currently-selected model.
#[derive(Clone)]
pub struct ModelEntry {
    pub provider: String,
    pub id: String,
    pub base_url: String,
    pub is_local: bool,
}

/// Whether a provider `baseUrl` points at this machine — the local/cloud
/// split behind both the picker label and the server dot.
fn is_local_base_url(base_url: &str) -> bool {
    let rest = base_url
        .strip_prefix("http://")
        .or_else(|| base_url.strip_prefix("https://"))
        .unwrap_or(base_url);
    let host = rest
        .split('/')
        .next()
        .unwrap_or("")
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or_else(|| rest.split('/').next().unwrap_or(""));
    matches!(
        host,
        "localhost" | "127.0.0.1" | "0.0.0.0" | "[::1]" | "::1"
    )
}

/// Picker label for one of pi's `Model` objects: name, provider, and either
/// "free · local" (local endpoint) or the per-Mtok in/out price when known.
fn model_label(m: &serde_json::Value) -> String {
    let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let name = m.get("name").and_then(|v| v.as_str()).unwrap_or(id);
    let provider = m.get("provider").and_then(|v| v.as_str()).unwrap_or("");
    let base_url = m.get("baseUrl").and_then(|v| v.as_str()).unwrap_or("");
    let mut label = if provider.is_empty() {
        name.to_string()
    } else {
        format!("{name} · {provider}")
    };
    if is_local_base_url(base_url) {
        label.push_str(" · free · local");
    } else {
        let price = |key: &str| m.pointer(&format!("/cost/{key}")).and_then(|v| v.as_f64());
        if let (Some(input), Some(output)) = (price("input"), price("output")) {
            let fmt = |v: f64| {
                let s = format!("{v:.2}");
                s.trim_end_matches('0').trim_end_matches('.').to_string()
            };
            label.push_str(&format!(" · ${}/${}", fmt(input), fmt(output)));
        }
    }
    label
}

/// `GetAvailableModels` -> `Vec<ModelRecord>` + the `GetState`-matched
/// current entry (for `compute_server_dot`) — ports `pi_core::backend::
/// refresh_models`. Every model-affecting action calls this uniformly
/// (rather than also porting `SetModel`'s local-patch shortcut): one code
/// path, no duplicated "locate by index" logic.
pub async fn refresh_models_and_state(client: &PiClient) -> (Vec<ModelRecord>, Option<ModelEntry>) {
    let mut entries = Vec::new();
    if let Ok(data) = client.get_available_models().await {
        if let Some(models) = data.get("models").and_then(|m| m.as_array()) {
            for m in models {
                let provider = m.get("provider").and_then(|v| v.as_str()).unwrap_or("");
                let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let base_url = m.get("baseUrl").and_then(|v| v.as_str()).unwrap_or("");
                entries.push((
                    ModelEntry {
                        provider: provider.to_string(),
                        id: id.to_string(),
                        base_url: base_url.to_string(),
                        is_local: is_local_base_url(base_url),
                    },
                    model_label(m),
                ));
            }
        }
    }
    let current_index = match client.get_state().await {
        Ok(state) => {
            let id = state.pointer("/model/id").and_then(|v| v.as_str());
            let provider = state.pointer("/model/provider").and_then(|v| v.as_str());
            entries.iter().position(|(e, _)| {
                Some(e.id.as_str()) == id && Some(e.provider.as_str()) == provider
            })
        }
        Err(_) => None,
    };
    let current_entry = current_index
        .and_then(|i| entries.get(i))
        .map(|(e, _)| e.clone());
    let records = entries
        .into_iter()
        .enumerate()
        .map(|(i, (entry, label))| ModelRecord {
            provider: entry.provider,
            id: entry.id,
            label,
            is_current: Some(i) == current_index,
        })
        .collect();
    (records, current_entry)
}

async fn probe_tcp(base_url: &str) -> bool {
    let https = base_url.starts_with("https://");
    let rest = base_url
        .strip_prefix("http://")
        .or_else(|| base_url.strip_prefix("https://"))
        .unwrap_or(base_url);
    let hostport = rest.split('/').next().unwrap_or("");
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().unwrap_or(if https { 443 } else { 80 })),
        None => (hostport, if https { 443 } else { 80 }),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::net::TcpStream::connect((host, port)),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false)
}

fn classify_rapid_mlx_dot(
    current_model_id: &str,
    health: Option<&pi_local::rapid_mlx::ServerHealth>,
    managed_alive: Option<bool>,
) -> ServerDotState {
    match health {
        Some(h) => {
            let serves_current = h.model_name.as_deref() == Some(current_model_id);
            if serves_current && h.ready && h.model_loaded {
                ServerDotState::Ok
            } else if serves_current {
                ServerDotState::Down
            } else {
                ServerDotState::Mismatch
            }
        }
        None => match managed_alive {
            Some(true) => ServerDotState::Ok,
            _ => ServerDotState::Down,
        },
    }
}

/// Status-bar dot state for the active model: hidden unless the model is
/// served from this machine. rapid-mlx providers get a real `/health`
/// probe (a plain reachability check would lie — see `ServerDotState::
/// Mismatch`'s doc comment); other local providers get a generic 1s TCP
/// probe; a managed child's process state breaks ties when the port
/// doesn't answer.
pub async fn compute_server_dot(
    current: Option<&ModelEntry>,
    managed_alive: Option<bool>,
) -> ServerDotState {
    let Some(entry) = current.filter(|e| e.is_local) else {
        return ServerDotState::Hidden;
    };
    if entry.provider == "rapid-mlx" {
        let health = pi_local::rapid_mlx::server_health(&entry.base_url).await;
        return classify_rapid_mlx_dot(&entry.id, health.as_ref(), managed_alive);
    }
    if probe_tcp(&entry.base_url).await {
        ServerDotState::Ok
    } else {
        ServerDotState::Down
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_free_label_has_no_price() {
        let m = serde_json::json!({
            "id": "llama-3", "name": "Llama 3", "provider": "rapid-mlx",
            "baseUrl": "http://127.0.0.1:8090/v1"
        });
        assert_eq!(model_label(&m), "Llama 3 · rapid-mlx · free · local");
    }

    #[test]
    fn cloud_priced_label_shows_trimmed_price() {
        let m = serde_json::json!({
            "id": "gpt-5", "name": "GPT-5", "provider": "openai",
            "baseUrl": "https://api.openai.com/v1",
            "cost": {"input": 3.0, "output": 15.25}
        });
        assert_eq!(model_label(&m), "GPT-5 · openai · $3/$15.25");
    }

    #[test]
    fn cloud_no_price_label_omits_suffix() {
        let m = serde_json::json!({
            "id": "x", "name": "X", "provider": "custom",
            "baseUrl": "https://example.com/v1"
        });
        assert_eq!(model_label(&m), "X · custom");
    }

    #[test]
    fn missing_name_falls_back_to_id() {
        let m = serde_json::json!({ "id": "bare-id", "provider": "", "baseUrl": "" });
        assert_eq!(model_label(&m), "bare-id");
    }

    #[test]
    fn serving_current_model_and_ready_is_ok() {
        let health = pi_local::rapid_mlx::ServerHealth {
            ready: true,
            model_loaded: true,
            model_name: Some("llama-3".to_string()),
        };
        assert_eq!(
            classify_rapid_mlx_dot("llama-3", Some(&health), None),
            ServerDotState::Ok
        );
    }

    #[test]
    fn serving_current_model_but_not_ready_is_down() {
        let health = pi_local::rapid_mlx::ServerHealth {
            ready: false,
            model_loaded: false,
            model_name: Some("llama-3".to_string()),
        };
        assert_eq!(
            classify_rapid_mlx_dot("llama-3", Some(&health), None),
            ServerDotState::Down
        );
    }

    #[test]
    fn serving_a_different_model_is_a_mismatch_not_ok() {
        let health = pi_local::rapid_mlx::ServerHealth {
            ready: true,
            model_loaded: true,
            model_name: Some("other-model".to_string()),
        };
        assert_eq!(
            classify_rapid_mlx_dot("llama-3", Some(&health), None),
            ServerDotState::Mismatch
        );
    }

    #[test]
    fn unreachable_falls_back_to_managed_alive() {
        assert_eq!(
            classify_rapid_mlx_dot("llama-3", None, Some(true)),
            ServerDotState::Ok
        );
        assert_eq!(
            classify_rapid_mlx_dot("llama-3", None, Some(false)),
            ServerDotState::Down
        );
        assert_eq!(
            classify_rapid_mlx_dot("llama-3", None, None),
            ServerDotState::Down
        );
    }
}
