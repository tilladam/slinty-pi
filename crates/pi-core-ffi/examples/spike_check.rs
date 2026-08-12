use pi_core_ffi::{
    ChatSink, ExtensionDialogRecord, ExtensionDialogReply, LocalModelIndex, PiSession, RowRecord,
    ServerDotState, SessionStatsRecord,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct PrintSink {
    chars_received: AtomicUsize,
    active_session: Mutex<Option<String>>,
    last_history: Mutex<Vec<RowRecord>>,
    dialogs: Mutex<Vec<ExtensionDialogRecord>>,
    pending_attachments: Mutex<Vec<String>>,
    composer_appends: Mutex<Vec<String>>,
}

impl ChatSink for PrintSink {
    fn on_text_delta(&self, delta: String) {
        self.chars_received
            .fetch_add(delta.chars().count(), Ordering::Relaxed);
        eprintln!("delta: {delta:?}");
    }
    fn on_turn_end(&self) {
        eprintln!("turn_end");
    }
    fn on_streaming_changed(&self, streaming: bool) {
        eprintln!("streaming: {streaming}");
    }
    fn on_error(&self, message: String) {
        eprintln!("error: {message}");
    }
    fn on_active_session_changed(&self, path: Option<String>) {
        eprintln!("active_session_changed: {path:?}");
        *self.active_session.lock().unwrap() = path;
    }
    fn on_history_replaced(&self, rows: Vec<RowRecord>) {
        let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
        eprintln!("history_replaced: {} rows, kinds={kinds:?}", rows.len());
        *self.last_history.lock().unwrap() = rows;
    }
    fn on_extension_dialog(&self, request: ExtensionDialogRecord) {
        eprintln!(
            "extension_dialog: method={} id={} title={:?} message={:?}",
            request.method, request.id, request.title, request.message
        );
        self.dialogs.lock().unwrap().push(request);
    }
    fn on_server_dot_changed(&self, state: ServerDotState) {
        eprintln!("server_dot_changed: {state:?}");
    }
    fn on_thinking_row_changed(&self, id: String, row: RowRecord) {
        eprintln!(
            "thinking_row_changed: id={id} running={} text={:?}",
            row.running, row.text
        );
    }
    fn on_tool_row_changed(&self, id: String, row: RowRecord) {
        eprintln!(
            "tool_row_changed: id={id} running={} elapsed={:?} text={:?}",
            row.running, row.elapsed, row.text
        );
    }
    fn on_session_stats_changed(&self, stats: SessionStatsRecord) {
        eprintln!(
            "session_stats_changed: tokens={} cost={} context_percent={}",
            stats.tokens_label, stats.cost, stats.context_percent
        );
    }
    fn on_pending_attachments_changed(&self, names: Vec<String>) {
        eprintln!("pending_attachments_changed: {names:?}");
        *self.pending_attachments.lock().unwrap() = names;
    }
    fn on_composer_append(&self, text: String) {
        eprintln!("composer_append: {text:?}");
        self.composer_appends.lock().unwrap().push(text);
    }
}

/// Manual verification for the real-`pi` acceptance points no Swift-side
/// build can exercise on its own (isolates the FFI/Rust layer from the
/// Xcode/UI layer):
///   cargo run -p pi-core-ffi --example spike_check              # round trip
///   cargo run -p pi-core-ffi --example spike_check -- abort     # abort mid-stream
///   cargo run -p pi-core-ffi --example spike_check -- sessions  # new/rename/delete session
///   cargo run -p pi-core-ffi --example spike_check -- history   # settle + switch_session hydration (SW3)
///   cargo run -p pi-core-ffi --example spike_check -- models    # LocalModelIndex refresh/search (SW4)
///   cargo run -p pi-core-ffi --example spike_check -- dialogs   # extension-UI dialog round trip (SW5)
///   cargo run -p pi-core-ffi --example spike_check -- live      # live thinking/tool-call preview (SW7)
///   cargo run -p pi-core-ffi --example spike_check -- thinking  # thinking-level list/set + stats fetch (SW8)
///   cargo run -p pi-core-ffi --example spike_check -- attach    # attach-path round trip, image + non-image (SW9)
///
/// The `dialogs` mode is tolerant of no gating extension being installed
/// (it just times out with a note, rather than failing) — real coverage
/// depends on whatever's actually installed under `~/.pi/agent/extensions/`
/// on the machine running it, same posture as the `models` mode below.
///
/// The `models` mode's rapid-mlx/router/Ollama checks are tolerant of the
/// underlying tool not being installed/running (matching this project's
/// convention, e.g. `local::hf`/`local::rapid_mlx`'s own live tests) —
/// only `search_hf_models` (a real network call to a public API with no
/// local dependency) is asserted on for real results. It also exercises
/// the composer picker's `PiSession.refresh_models`/`set_model` (SW6)
/// against whatever models `pi`'s own config already lists.
///
/// The abort mode's outcome depends on how the configured pi model/
/// extensions handle a "write something long" prompt: a plain conversational
/// reply streams as `TextDelta`s (the case this asserts on), but some
/// setups route longer requests through a tool call or planning step first
/// — invisible to this crate's deliberately minimal `ChatSink` (see the
/// crate doc comment) — in which case `before`/`after` both stay at 0 and
/// the assertion is a no-op rather than a false failure. Either way,
/// `abort()` itself is exercised against a real running child with no
/// crash/hang, which is the actual thing under test.
fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let mode = std::env::args().nth(1);

    eprintln!("creating PiSession...");
    let sink = Arc::new(PrintSink {
        chars_received: AtomicUsize::new(0),
        active_session: Mutex::new(None),
        last_history: Mutex::new(Vec::new()),
        dialogs: Mutex::new(Vec::new()),
        pending_attachments: Mutex::new(Vec::new()),
        composer_appends: Mutex::new(Vec::new()),
    });
    let cwd = std::env::current_dir()
        .expect("current dir")
        .display()
        .to_string();
    let session = PiSession::new(sink.clone(), cwd, None, true).expect("PiSession::new failed");
    // Let the constructor's initial on_active_session_changed land before
    // any mode below reads sink.active_session.
    std::thread::sleep(Duration::from_millis(200));

    match mode.as_deref() {
        Some("abort") => {
            eprintln!("PiSession created, sending a long prompt then aborting mid-stream");
            session.send("say hi in exactly 200 words".to_string());
            std::thread::sleep(Duration::from_millis(60));
            let before = sink.chars_received.load(Ordering::Relaxed);
            eprintln!("--- calling abort() after {before} chars streamed ---");
            session.abort();
            std::thread::sleep(Duration::from_secs(2));
            let after = sink.chars_received.load(Ordering::Relaxed);
            eprintln!("chars streamed after abort() returned: {after}");
            assert!(
                after < before + 200,
                "expected abort to cut a 200-word reply off well before it finished \
                 ({before} chars before abort, {after} chars total)"
            );
        }
        Some("sessions") => {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            rt.block_on(async {
                eprintln!("--- new_session() ---");
                session.new_session().await.expect("new_session failed");
                eprintln!(
                    "active session after new_session: {:?}",
                    sink.active_session.lock().unwrap()
                );

                eprintln!("--- sending a prompt so pi writes the session file ---");
                session.send("say hi in exactly 3 words".to_string());
                tokio::time::sleep(Duration::from_secs(5)).await;

                eprintln!("--- rename_session() ---");
                session
                    .rename_session("spike-check test session".to_string())
                    .await
                    .expect("rename_session failed");
                eprintln!("rename ok");

                let path = sink.active_session.lock().unwrap().clone();
                match path {
                    Some(path) => {
                        eprintln!("--- delete_session({path}) ---");
                        session
                            .delete_session(path)
                            .await
                            .expect("delete_session failed");
                        eprintln!(
                            "active session after delete_session: {:?}",
                            sink.active_session.lock().unwrap()
                        );
                    }
                    None => eprintln!("no active session path known — skipping delete_session"),
                }
            });
        }
        Some("history") => {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            rt.block_on(async {
                eprintln!("--- sending a prompt that should produce a code block ---");
                session.send(
                    "Reply with only a fenced rust code block containing exactly: fn main() {}"
                        .to_string(),
                );
                // Generous margin: real model latency, not a fixed cadence
                // like the demo streamer. AgentSettled -> hydrate_and_push
                // is what should populate last_history.
                tokio::time::sleep(Duration::from_secs(20)).await;

                let after_send = sink.last_history.lock().unwrap().clone();
                let kinds_after_send: Vec<String> =
                    after_send.iter().map(|r| r.kind.clone()).collect();
                eprintln!("history after send/settle: {kinds_after_send:?}");
                assert!(
                    !after_send.is_empty(),
                    "expected on_history_replaced to have fired after the turn settled"
                );

                let path = sink
                    .active_session
                    .lock()
                    .unwrap()
                    .clone()
                    .expect("expected an active session path after sending a prompt");

                eprintln!(
                    "--- switch_session({path}) to verify resume reproduces the same render ---"
                );
                session
                    .switch_session(path)
                    .await
                    .expect("switch_session failed");

                let after_resume = sink.last_history.lock().unwrap().clone();
                let kinds_after_resume: Vec<String> =
                    after_resume.iter().map(|r| r.kind.clone()).collect();
                eprintln!("history after switch_session: {kinds_after_resume:?}");
                assert_eq!(
                    kinds_after_send, kinds_after_resume,
                    "resuming the same session should render the same row kinds the live settle did"
                );
                assert!(
                    after_resume.iter().any(|r| r.kind == "code"),
                    "expected at least one code row, got kinds {kinds_after_resume:?}"
                );
            });
        }
        Some("models") => {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            rt.block_on(async {
                let index = LocalModelIndex::new();

                eprintln!("--- refresh_rapid_mlx_panel() ---");
                let rapid_mlx = index.refresh_rapid_mlx_panel().await;
                eprintln!(
                    "rapid-mlx: version={:?} running_summary={:?} cached={} catalog_count={}",
                    rapid_mlx.version,
                    rapid_mlx.running_summary,
                    rapid_mlx.cached.len(),
                    rapid_mlx.catalog_count
                );

                eprintln!("--- refresh_router_panel() ---");
                let router = index.refresh_router_panel().await;
                eprintln!(
                    "router: status={} base_url={} models={}",
                    router.status_label,
                    router.base_url,
                    router.models.len()
                );

                eprintln!("--- refresh_ollama_panel() ---");
                let ollama = index.refresh_ollama_panel().await;
                eprintln!(
                    "ollama: detected={} summary={:?} model_count={}",
                    ollama.detected, ollama.summary, ollama.model_count
                );

                eprintln!("--- refresh_auth_entries() ---");
                let auth = index.refresh_auth_entries().await;
                eprintln!("auth entries: {auth:?}");

                eprintln!("--- search_hf_models(\"phi-4\") ---");
                let results = index
                    .search_hf_models("phi-4".to_string())
                    .await
                    .expect("hf search failed");
                eprintln!("hf search: {} results", results.len());
                assert!(
                    !results.is_empty(),
                    "expected at least one real HF search result for 'phi-4'"
                );
                let first = &results[0];
                eprintln!(
                    "first result: {} gated={} downloads={} quants={:?}",
                    first.id, first.gated, first.downloads, first.quants
                );

                eprintln!("--- session.refresh_models() ---");
                let models = session
                    .refresh_models()
                    .await
                    .expect("refresh_models failed");
                eprintln!("models: {} entries", models.len());
                for m in &models {
                    eprintln!(
                        "  {}{} — {}",
                        if m.is_current { "* " } else { "  " },
                        m.label,
                        m.id
                    );
                }
                if let Some(current) = models.iter().find(|m| m.is_current) {
                    eprintln!("--- session.set_model() re-selecting the current model ---");
                    session
                        .set_model(current.provider.clone(), current.id.clone())
                        .await
                        .expect("set_model failed");
                    eprintln!("set_model ok — no hang");
                } else {
                    eprintln!(
                        "no current model reported by get_state — skipping set_model round trip"
                    );
                }
            });
        }
        Some("dialogs") => {
            eprintln!(
                "--- sending a bash command likely to trigger an installed gating extension ---"
            );
            session.send("Run the bash command: echo spike-check-dialog-verification".to_string());
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let dialog = sink.dialogs.lock().unwrap().first().cloned();
                match dialog {
                    Some(dialog) => {
                        eprintln!(
                            "--- extension dialog observed: method={} id={} ---",
                            dialog.method, dialog.id
                        );
                        assert!(
                            matches!(dialog.method.as_str(), "confirm" | "select"),
                            "expected a confirm or select dialog, got {}",
                            dialog.method
                        );
                        session.reply_extension_dialog(
                            dialog.id.clone(),
                            ExtensionDialogReply::Cancelled,
                        );
                        eprintln!("replied Cancelled to {} — no hang", dialog.id);
                        break;
                    }
                    None if Instant::now() > deadline => {
                        eprintln!(
                            "no extension_ui_request observed within 30s — no gating extension \
                             installed (or this command was already allow-listed); nothing to \
                             verify"
                        );
                        break;
                    }
                    None => std::thread::sleep(Duration::from_millis(100)),
                }
            }
        }
        Some("live") => {
            eprintln!(
                "--- sending a prompt likely to trigger a real tool call, watching for live \
                 thinking_row_changed/tool_row_changed events (SW7) — see PrintSink's eprintln \
                 output above for the actual events observed ---"
            );
            session.send(
                "Read Cargo.toml in the current directory and summarize it in one sentence."
                    .to_string(),
            );
            std::thread::sleep(Duration::from_secs(20));
            eprintln!(
                "--- done watching — confirm above that at least one tool_row_changed fired \
                 with running=true before the (eventual) running=false, i.e. it was visible \
                 while the turn was still in flight, not just after ---"
            );
        }
        Some("thinking") => {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            rt.block_on(async {
                eprintln!("--- session.refresh_thinking_levels() ---");
                let levels = session
                    .refresh_thinking_levels()
                    .await
                    .expect("refresh_thinking_levels failed");
                eprintln!("thinking levels: {} entries", levels.len());
                for l in &levels {
                    eprintln!("  {}{}", if l.is_current { "* " } else { "  " }, l.label);
                }
                if let Some(current) = levels.iter().find(|l| l.is_current) {
                    eprintln!(
                        "--- session.set_thinking_level() re-selecting the current level ---"
                    );
                    session
                        .set_thinking_level(current.level)
                        .await
                        .expect("set_thinking_level failed");
                    eprintln!("set_thinking_level ok — no hang");
                } else {
                    eprintln!(
                        "no thinking levels reported (or none matched GetState) — skipping \
                         set_thinking_level round trip"
                    );
                }
            });
            eprintln!(
                "--- sending a prompt so a real turn settles, watching for \
                 session_stats_changed (SW8) — see PrintSink's eprintln output above for the \
                 actual snapshot pushed via hydrate_and_push ---"
            );
            session.send("say hi in exactly 3 words".to_string());
            std::thread::sleep(Duration::from_secs(15));
        }
        Some("attach") => {
            let dir = std::env::temp_dir();

            eprintln!("--- attach_path() on a fixture image ---");
            let image_path = dir.join("spike-check-attach.png");
            std::fs::write(&image_path, b"not a real png, just bytes to base64-encode")
                .expect("write fixture image");
            session.attach_path(image_path.display().to_string());
            std::thread::sleep(Duration::from_secs(2));
            let names = sink.pending_attachments.lock().unwrap().clone();
            eprintln!("pending attachments after attach_path: {names:?}");
            assert_eq!(
                names,
                vec!["spike-check-attach.png".to_string()],
                "expected the fixture image's file name to appear as a pending attachment"
            );

            eprintln!("--- attach_path() on a non-image file (should append @path instead) ---");
            let text_path = dir.join("spike-check-attach.txt");
            std::fs::write(&text_path, b"not an image").expect("write fixture text file");
            session.attach_path(text_path.display().to_string());
            std::thread::sleep(Duration::from_secs(2));
            let appends = sink.composer_appends.lock().unwrap().clone();
            eprintln!("composer appends after non-image attach_path: {appends:?}");
            assert_eq!(
                appends,
                vec![text_path.display().to_string()],
                "expected the non-image path to be pushed via on_composer_append, unchanged"
            );
            // The non-image attach shouldn't have touched the image queue.
            assert_eq!(sink.pending_attachments.lock().unwrap().clone(), names);

            eprintln!(
                "--- sending a prompt so the queued image attaches — watching for \
                 pending_attachments_changed to clear (see PrintSink's eprintln output above) ---"
            );
            session.send("what's in the attached file? Reply in one sentence.".to_string());
            std::thread::sleep(Duration::from_secs(15));
            let names_after_send = sink.pending_attachments.lock().unwrap().clone();
            eprintln!("pending attachments after send: {names_after_send:?}");
            assert!(
                names_after_send.is_empty(),
                "expected the attachment queue to clear once consumed by send"
            );

            let _ = std::fs::remove_file(&image_path);
            let _ = std::fs::remove_file(&text_path);
        }
        _ => {
            eprintln!("PiSession created, sending prompt");
            session.send("say hi in exactly 3 words".to_string());
            std::thread::sleep(Duration::from_secs(8));
        }
    }
    eprintln!("done");
}
