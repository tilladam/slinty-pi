use pi_core_ffi::{ChatSink, PiSession};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct PrintSink {
    chars_received: AtomicUsize,
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
}

/// Manual verification for SW1's two real-`pi` acceptance points (no Swift
/// involved — isolates the FFI/Rust layer from the Xcode/UI layer):
///   cargo run -p pi-core-ffi --example spike_check           # round trip
///   cargo run -p pi-core-ffi --example spike_check -- abort  # abort mid-stream
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
    let test_abort = std::env::args().nth(1).as_deref() == Some("abort");

    eprintln!("creating PiSession...");
    let sink = Arc::new(PrintSink {
        chars_received: AtomicUsize::new(0),
    });
    let session = PiSession::new(sink.clone()).expect("PiSession::new failed");

    if test_abort {
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
    } else {
        eprintln!("PiSession created, sending prompt");
        session.send("say hi in exactly 3 words".to_string());
        std::thread::sleep(Duration::from_secs(8));
    }
    eprintln!("done");
}
