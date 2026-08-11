//! Local-model backend foundation: the llama.cpp router HTTP client,
//! rapid-mlx CLI integration, Hugging Face GGUF search, Ollama detection,
//! `~/.pi/agent/{auth,models}.json` handling, and system RAM-fit
//! estimation. Toolkit-agnostic and frontend-agnostic — no `Transcript`/
//! `UiSink`/`PiClient` coupling anywhere in this crate. Shared by `pi-core`
//! (Slint's live models panel) and `pi-core-ffi` (Swift's `LocalModelIndex`)
//! so neither reimplements this HTTP/CLI/file-I/O logic a second time.
//!
//! `rapid_mlx`/`system_fit` back the rapid-mlx section; `router` backs the
//! llama.cpp router section — list/load/unload with progress polled from
//! `GET /models`. `download_model` backs `hf`'s "Download model…" flow;
//! `delete_model`/the SSE stream aren't used yet — see their own
//! `#[allow(dead_code)]` markers.
//!
//! `panel` holds the pure "collect + format" pipeline that turns these
//! clients' output into the plain row/summary data both frontends' models
//! panels render, plus deterministic offline demo/test fixtures run through
//! those same formatters.

pub mod auth_json;
pub mod hf;
pub mod models_json;
pub mod ollama;
pub mod panel;
pub mod rapid_mlx;
pub mod router;
pub mod system_fit;
