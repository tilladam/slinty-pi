//! Pure/impure "collect + format" pipeline turning `rapid_mlx`/`router`/
//! `hf`/`ollama`/`auth_json`'s client output into the plain row/summary
//! data both frontends' models panels render — kept separate from those
//! clients so a frontend can call the (comparatively expensive, multi-
//! shellout) rapid-mlx collection step independently of a cheap router-only
//! refresh (a load/unload/download poll loop must not re-scan rapid-mlx on
//! every tick).
//!
//! The `seed_demo_*`/`demo_rapid_mlx_snapshot` fixtures exist so a frontend
//! can demo this panel deterministically, offline, without whatever happens
//! to be installed/cached on the machine running the demo — and so that
//! demoing it exercises the exact same formatters the live path uses, not a
//! parallel stand-in. Both this crate's own tests and `pi-core`'s Slint demo
//! backend use them.

use std::collections::HashMap;

use crate::{auth_json, hf, ollama, rapid_mlx, router};

/// The port a managed `rapid-mlx serve` child listens on. One model per
/// process (no hot-swap) — a model switch stops the previous managed server
/// and starts a new one on this same port.
pub const RAPID_MLX_PORT: u16 = 8000;

/// UI-ready rapid-mlx panel data. `cached` rows are `(alias, hf_repo,
/// human_size, fit_label)`.
#[derive(Debug, Clone, PartialEq)]
pub struct RapidMlxPanelData {
    pub version: Option<String>,
    pub running_summary: Option<String>,
    pub cached: Vec<(String, String, String, String)>,
    pub catalog_count: usize,
}

/// UI-ready router panel data. `models` rows are `(id, status_label, loaded,
/// busy)`.
#[derive(Debug, Clone, PartialEq)]
pub struct RouterPanelData {
    pub status_label: String,
    pub base_url: String,
    pub models: Vec<(String, String, bool, bool)>,
}

/// Raw rapid-mlx CLI results, collected in one shot. Kept separate from
/// [`RapidMlxPanelData`] so a demo/test fixture can seed a fake snapshot and
/// run it through the same [`format_rapid_mlx_panel`] the live path uses.
pub struct RapidMlxSnapshot {
    pub version: Option<String>,
    pub running: Vec<rapid_mlx::RunningServer>,
    pub cached: Vec<rapid_mlx::CachedModel>,
    pub catalog_count: usize,
}

pub async fn collect_rapid_mlx_snapshot() -> RapidMlxSnapshot {
    let rmlx = rapid_mlx::RapidMlx::default();
    let version = rmlx.version().await;
    let running = rmlx.running_servers().await.unwrap_or_default();
    let cached = rmlx.cached_models().await.unwrap_or_default();
    let catalog_count = rmlx.catalog().await.map(|c| c.len()).unwrap_or(0);
    RapidMlxSnapshot {
        version,
        running,
        cached,
        catalog_count,
    }
}

pub fn format_rapid_mlx_panel(
    snapshot: RapidMlxSnapshot,
    mem: &crate::system_fit::SystemMemory,
) -> RapidMlxPanelData {
    let running_summary = snapshot
        .running
        .first()
        .map(|s| format!("{} running on :{} (uptime {})", s.model, s.port, s.uptime));
    let cached = snapshot
        .cached
        .into_iter()
        .map(|c| {
            let fit = mem.fit_label_for(c.size_bytes).label().to_string();
            (
                c.alias,
                c.hf_repo,
                crate::system_fit::human_size(c.size_bytes),
                fit,
            )
        })
        .collect();
    RapidMlxPanelData {
        version: snapshot.version,
        running_summary,
        cached,
        catalog_count: snapshot.catalog_count,
    }
}

pub fn router_health_label(health: router::HealthState) -> &'static str {
    match health {
        router::HealthState::Ready => "ready",
        router::HealthState::Loading => "loading",
        router::HealthState::Unreachable => "unreachable",
    }
}

/// Human status label for one router model row, including progress when
/// loading/downloading (`/models` carries `status.progress` directly, so
/// polling this endpoint is enough to show live progress without also
/// wiring the `/models/sse` stream — see the module-level design note).
pub fn router_model_status_label(status: &router::ModelStatus) -> String {
    use router::{StatusProgress, StatusValue};
    if status.failed {
        return match status.exit_code {
            Some(code) => format!("failed (exit {code})"),
            None => "failed".to_string(),
        };
    }
    match status.value {
        StatusValue::Loaded => "loaded".to_string(),
        StatusValue::Unloaded => "unloaded".to_string(),
        StatusValue::Sleeping => "sleeping".to_string(),
        StatusValue::Loading => match &status.progress {
            Some(StatusProgress::Loading(p)) => match (&p.current, p.value) {
                (Some(stage), Some(v)) => format!("loading {stage} {:.0}%", v * 100.0),
                (None, Some(v)) => format!("loading {:.0}%", v * 100.0),
                _ => "loading…".to_string(),
            },
            _ => "loading…".to_string(),
        },
        StatusValue::Downloading => match &status.progress {
            Some(StatusProgress::Downloading(files)) if !files.is_empty() => {
                let (done, total) = files
                    .values()
                    .fold((0u64, 0u64), |(d, t), f| (d + f.done, t + f.total));
                if total > 0 {
                    format!("downloading {:.0}%", done as f64 / total as f64 * 100.0)
                } else {
                    "downloading…".to_string()
                }
            }
            _ => "downloading…".to_string(),
        },
        StatusValue::Unknown => "unknown".to_string(),
    }
}

pub fn router_model_busy(status: &router::ModelStatus) -> bool {
    matches!(
        status.value,
        router::StatusValue::Loading | router::StatusValue::Downloading
    )
}

/// Pure `/models` entries -> UI rows. Shared by the live path
/// (`fetch_router_state`) and demo/test fixtures — see the module-level
/// design note on why this must not be duplicated.
pub fn format_router_models(entries: Vec<router::ModelEntry>) -> Vec<(String, String, bool, bool)> {
    entries
        .into_iter()
        .map(|e| {
            let label = router_model_status_label(&e.status);
            let loaded = matches!(e.status.value, router::StatusValue::Loaded);
            let busy = router_model_busy(&e.status);
            (e.id, label, loaded, busy)
        })
        .collect()
}

pub async fn fetch_router_state(router: &router::LlamaRouter) -> RouterPanelData {
    let health = router.health().await;
    let models = if health == crate::router::HealthState::Unreachable {
        Vec::new()
    } else {
        router
            .list_models(false)
            .await
            .map(format_router_models)
            .unwrap_or_default()
    };
    RouterPanelData {
        status_label: router_health_label(health).to_string(),
        base_url: router.base_url().to_string(),
        models,
    }
}

/// Pure entries -> panel labels, shared by the live path and demo/test
/// fixtures. Only provider ids and form labels — key material never reaches
/// the UI model.
pub fn format_auth_entries(entries: &[(String, auth_json::KeyForm)]) -> Vec<String> {
    entries
        .iter()
        .map(|(provider, form)| format!("{provider} · {}", form.label()))
        .collect()
}

/// Pure Ollama models -> panel summary, shared by the live path and demo/
/// test fixtures — same shared-formatter guarantee as the router/HF
/// formatters. `None` means undetected (not installed, not running — the
/// panel doesn't distinguish why, see `OllamaProbe`).
pub fn format_ollama_panel(models: Option<Vec<ollama::OllamaModel>>) -> (bool, String, i32) {
    match models {
        None => (false, String::new(), 0),
        Some(models) if models.is_empty() => {
            (true, "detected, no models pulled yet".to_string(), 0)
        }
        Some(models) => {
            let count = models.len();
            let names = models
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            (true, format!("{count} model(s): {names}"), count as i32)
        }
    }
}

/// Pure Hugging Face search results -> UI rows, shared by the live path and
/// demo/test fixtures — same shared-formatter guarantee as
/// `format_router_models`.
pub fn format_hf_results(models: Vec<hf::HfModel>) -> Vec<(String, bool, i32, Vec<String>)> {
    models
        .into_iter()
        .map(|m| {
            let quants = hf::gguf_quants(&m);
            (m.id, m.gated.is_gated(), m.downloads as i32, quants)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Demo/test fixtures: deterministic, offline data run through the same
// formatters the live path uses.
// ---------------------------------------------------------------------------

pub fn seed_demo_auth_entries() -> Vec<(String, auth_json::KeyForm)> {
    use auth_json::KeyForm;
    vec![
        ("anthropic".to_string(), KeyForm::Literal),
        ("cloudflare-ai-gateway".to_string(), KeyForm::Env),
        ("openai".to_string(), KeyForm::Command),
        ("github-copilot".to_string(), KeyForm::Managed),
    ]
}

pub fn demo_rapid_mlx_snapshot() -> RapidMlxSnapshot {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    RapidMlxSnapshot {
        version: Some("rapid-mlx 0.11.0".to_string()),
        running: vec![rapid_mlx::RunningServer {
            pid: 12345,
            port: RAPID_MLX_PORT,
            model: "mlx-community/Qwen3.5-4B-MLX-4bit".to_string(),
            uptime: "12m".to_string(),
        }],
        cached: vec![
            rapid_mlx::CachedModel {
                alias: "qwen3.5-4b-4bit".to_string(),
                hf_repo: "mlx-community/Qwen3.5-4B-MLX-4bit".to_string(),
                size_bytes: (5.7 * GIB) as u64,
                modified: "2d ago".to_string(),
            },
            rapid_mlx::CachedModel {
                alias: "gpt-oss-120b".to_string(),
                hf_repo: "mlx-community/gpt-oss-120b-MXFP4-Q8".to_string(),
                size_bytes: (118.1 * GIB) as u64,
                modified: "1d ago".to_string(),
            },
        ],
        catalog_count: 165,
    }
}

/// Seeds one router model in each state the panel can render — loaded,
/// unloaded, loading (with progress), downloading (with progress), and
/// failed — so a single render exercises every branch of
/// `router_model_status_label` at once.
pub fn seed_demo_router_entries() -> Vec<router::ModelEntry> {
    use router::{
        FileProgress, LoadingProgress, ModelEntry, ModelStatus, StatusProgress, StatusValue,
    };
    vec![
        ModelEntry {
            id: "ggml-org/gemma-3-4b-it-GGUF:Q4_K_M".to_string(),
            path: Some("/demo/gemma-3-4b-it.gguf".to_string()),
            status: ModelStatus {
                value: StatusValue::Loaded,
                args: vec!["llama-server".to_string()],
                failed: false,
                exit_code: None,
                progress: None,
            },
            architecture: None,
        },
        ModelEntry {
            id: "unsloth/Qwen3-8B-GGUF:Q4_K_M".to_string(),
            path: Some("/demo/qwen3-8b.gguf".to_string()),
            status: ModelStatus {
                value: StatusValue::Unloaded,
                args: vec![],
                failed: false,
                exit_code: None,
                progress: None,
            },
            architecture: None,
        },
        ModelEntry {
            id: "mlx-community/Llama-3.2-3B-Instruct-4bit".to_string(),
            path: None,
            status: ModelStatus {
                value: StatusValue::Loading,
                args: vec![],
                failed: false,
                exit_code: None,
                progress: Some(StatusProgress::Loading(LoadingProgress {
                    stages: vec!["text_model".to_string()],
                    current: Some("text_model".to_string()),
                    value: Some(0.45),
                })),
            },
            architecture: None,
        },
        ModelEntry {
            id: "TheBloke/Mistral-7B-Instruct-v0.2-GGUF:Q4_K_M".to_string(),
            path: None,
            status: ModelStatus {
                value: StatusValue::Downloading,
                args: vec![],
                failed: false,
                exit_code: None,
                progress: Some(StatusProgress::Downloading(HashMap::from([(
                    "https://huggingface.co/TheBloke/Mistral-7B-Instruct-v0.2-GGUF/model.gguf"
                        .to_string(),
                    FileProgress {
                        done: 60,
                        total: 100,
                    },
                )]))),
            },
            architecture: None,
        },
        ModelEntry {
            id: "broken/does-not-load-GGUF".to_string(),
            path: None,
            status: ModelStatus {
                value: StatusValue::Unloaded,
                args: vec!["llama-server".to_string()],
                failed: true,
                exit_code: Some(1),
                progress: None,
            },
            architecture: None,
        },
    ]
}

/// Two seeded Ollama models — deterministic, offline, same reasoning as
/// `demo_rapid_mlx_snapshot`/`seed_demo_router_entries`.
pub fn seed_demo_ollama_models() -> Vec<ollama::OllamaModel> {
    vec![
        ollama::OllamaModel {
            name: "llama3.1:8b".to_string(),
            size: 4_920_000_000,
            details: None,
        },
        ollama::OllamaModel {
            name: "qwen2.5-coder:7b".to_string(),
            size: 4_680_000_000,
            details: None,
        },
    ]
}

/// Two seeded HF search results (one gated, one not) covering the quant-chip
/// and gated-warning rendering paths — run through the same
/// `format_hf_results` the live path uses.
pub fn seed_demo_hf_results() -> Vec<hf::HfModel> {
    use hf::{Gated, HfModel, Sibling};
    fn siblings(names: &[&str]) -> Vec<Sibling> {
        names
            .iter()
            .map(|n| Sibling {
                rfilename: n.to_string(),
            })
            .collect()
    }
    vec![
        HfModel {
            id: "unsloth/Phi-4-mini-instruct-GGUF".to_string(),
            gated: Gated::Bool(false),
            downloads: 48213,
            siblings: siblings(&[
                "Phi-4-mini-instruct-BF16.gguf",
                "Phi-4-mini-instruct-Q4_K_M.gguf",
                "Phi-4-mini-instruct-Q8_0.gguf",
            ]),
        },
        HfModel {
            id: "meta-llama/Llama-3.1-8B-Instruct-GGUF".to_string(),
            gated: Gated::Kind("manual".to_string()),
            downloads: 1523890,
            siblings: siblings(&[
                "Llama-3.1-8B-Instruct-Q4_K_M.gguf",
                "Llama-3.1-8B-Instruct-Q5_K_M.gguf",
            ]),
        },
    ]
}

#[cfg(test)]
mod auth_panel_tests {
    use super::*;

    #[test]
    fn labels_cover_every_form_and_never_contain_key_material() {
        let labels = format_auth_entries(&seed_demo_auth_entries());
        assert_eq!(
            labels,
            vec![
                "anthropic · api key",
                "cloudflare-ai-gateway · $ENV — read-only",
                "openai · !command — read-only",
                "github-copilot · managed by pi /login",
            ]
        );
    }
}

#[cfg(test)]
mod models_panel_tests {
    use super::*;
    use crate::router::{FileProgress, LoadingProgress, ModelStatus, StatusProgress, StatusValue};

    #[test]
    fn status_label_covers_every_branch() {
        let loaded = ModelStatus {
            value: StatusValue::Loaded,
            args: vec![],
            failed: false,
            exit_code: None,
            progress: None,
        };
        assert_eq!(router_model_status_label(&loaded), "loaded");

        let loading_with_stage = ModelStatus {
            value: StatusValue::Loading,
            args: vec![],
            failed: false,
            exit_code: None,
            progress: Some(StatusProgress::Loading(LoadingProgress {
                stages: vec![],
                current: Some("text_model".to_string()),
                value: Some(0.45),
            })),
        };
        assert_eq!(
            router_model_status_label(&loading_with_stage),
            "loading text_model 45%"
        );

        let downloading = ModelStatus {
            value: StatusValue::Downloading,
            args: vec![],
            failed: false,
            exit_code: None,
            progress: Some(StatusProgress::Downloading(HashMap::from([(
                "https://x/model.gguf".to_string(),
                FileProgress {
                    done: 60,
                    total: 100,
                },
            )]))),
        };
        assert_eq!(router_model_status_label(&downloading), "downloading 60%");

        let failed = ModelStatus {
            value: StatusValue::Unloaded,
            args: vec![],
            failed: true,
            exit_code: Some(1),
            progress: None,
        };
        assert_eq!(router_model_status_label(&failed), "failed (exit 1)");
    }

    #[test]
    fn busy_is_true_only_while_loading_or_downloading() {
        let loading = ModelStatus {
            value: StatusValue::Loading,
            args: vec![],
            failed: false,
            exit_code: None,
            progress: None,
        };
        assert!(router_model_busy(&loading));

        let loaded = ModelStatus {
            value: StatusValue::Loaded,
            args: vec![],
            failed: false,
            exit_code: None,
            progress: None,
        };
        assert!(!router_model_busy(&loaded));
    }

    /// The demo/test fixture catalog and the live path's real `/models`
    /// response both flow through `format_router_models` — this is the
    /// guarantee that verifying the demo panel actually exercises the
    /// formatter the live path runs, not a parallel stand-in.
    #[test]
    fn seeded_demo_entries_cover_every_row_state_via_the_shared_formatter() {
        let rows = format_router_models(seed_demo_router_entries());
        assert_eq!(rows.len(), 5);

        let (_, status, loaded, busy) = rows.iter().find(|(id, ..)| id.contains("gemma")).unwrap();
        assert_eq!(status, "loaded");
        assert!(loaded);
        assert!(!busy);

        let (_, status, loaded, busy) = rows
            .iter()
            .find(|(id, ..)| id.contains("Qwen3-8B"))
            .unwrap();
        assert_eq!(status, "unloaded");
        assert!(!loaded);
        assert!(!busy);

        let (_, status, loaded, busy) = rows
            .iter()
            .find(|(id, ..)| id.contains("Llama-3.2"))
            .unwrap();
        assert!(status.starts_with("loading"));
        assert!(status.contains('%'));
        assert!(!loaded);
        assert!(busy);

        let (_, status, loaded, busy) =
            rows.iter().find(|(id, ..)| id.contains("Mistral")).unwrap();
        assert!(status.starts_with("downloading"));
        assert!(status.contains('%'));
        assert!(!loaded);
        assert!(busy);

        let (_, status, loaded, busy) = rows.iter().find(|(id, ..)| id.contains("broken")).unwrap();
        assert!(status.starts_with("failed"));
        assert!(!loaded);
        assert!(!busy);
    }

    #[test]
    fn demo_rapid_mlx_snapshot_formats_into_a_fit_labeled_cached_row() {
        let mem = crate::system_fit::SystemMemory {
            total_bytes: 32 * 1024 * 1024 * 1024,
            available_bytes: 32 * 1024 * 1024 * 1024,
        };
        let data = format_rapid_mlx_panel(demo_rapid_mlx_snapshot(), &mem);
        assert_eq!(data.version.as_deref(), Some("rapid-mlx 0.11.0"));
        assert!(data.running_summary.unwrap().contains("Qwen3.5-4B"));
        assert_eq!(data.cached.len(), 2);
        let (alias, hf_repo, size, fit) = &data.cached[0];
        assert_eq!(alias, "qwen3.5-4b-4bit");
        assert_eq!(hf_repo, "mlx-community/Qwen3.5-4B-MLX-4bit");
        assert_eq!(size, "5.7 GiB");
        assert_eq!(fit, "Fits");
    }

    /// Same shared-formatter guarantee as the router fixture test above,
    /// for the HF search results path.
    #[test]
    fn seeded_demo_hf_results_cover_gated_and_public_via_the_shared_formatter() {
        let rows = format_hf_results(seed_demo_hf_results());
        assert_eq!(rows.len(), 2);

        let (id, gated, downloads, quants) =
            rows.iter().find(|(id, ..)| id.contains("Phi-4")).unwrap();
        assert_eq!(id, "unsloth/Phi-4-mini-instruct-GGUF");
        assert!(!gated);
        assert!(*downloads > 0);
        assert_eq!(
            quants,
            &vec!["BF16".to_string(), "Q4_K_M".to_string(), "Q8_0".to_string()]
        );

        let (_, gated, _, quants) = rows
            .iter()
            .find(|(id, ..)| id.contains("Llama-3.1"))
            .unwrap();
        assert!(gated);
        assert_eq!(quants, &vec!["Q4_K_M".to_string(), "Q5_K_M".to_string()]);
    }

    #[test]
    fn ollama_panel_distinguishes_undetected_empty_and_populated() {
        let (detected, summary, count) = format_ollama_panel(None);
        assert!(!detected);
        assert_eq!(count, 0);
        assert!(summary.is_empty());

        let (detected, summary, count) = format_ollama_panel(Some(Vec::new()));
        assert!(detected);
        assert_eq!(count, 0);
        assert_eq!(summary, "detected, no models pulled yet");

        let (detected, summary, count) = format_ollama_panel(Some(seed_demo_ollama_models()));
        assert!(detected);
        assert_eq!(count, 2);
        assert!(summary.contains("llama3.1:8b"));
        assert!(summary.contains("qwen2.5-coder:7b"));
    }
}
