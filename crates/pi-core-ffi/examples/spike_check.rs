use pi_core_ffi::{ChatSink, PiSession, RowRecord};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct PrintSink {
    chars_received: AtomicUsize,
    active_session: Mutex<Option<String>>,
    last_history: Mutex<Vec<RowRecord>>,
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
}

/// Manual verification for the real-`pi` acceptance points no Swift-side
/// build can exercise on its own (isolates the FFI/Rust layer from the
/// Xcode/UI layer):
///   cargo run -p pi-core-ffi --example spike_check              # round trip
///   cargo run -p pi-core-ffi --example spike_check -- abort     # abort mid-stream
///   cargo run -p pi-core-ffi --example spike_check -- sessions  # new/rename/delete session
///   cargo run -p pi-core-ffi --example spike_check -- history   # settle + switch_session hydration (SW3)
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
        _ => {
            eprintln!("PiSession created, sending prompt");
            session.send("say hi in exactly 3 words".to_string());
            std::thread::sleep(Duration::from_secs(8));
        }
    }
    eprintln!("done");
}
