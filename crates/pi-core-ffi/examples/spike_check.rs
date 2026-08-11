use pi_core_ffi::{ChatSink, PiSession};
use std::sync::Arc;
use std::time::Duration;

struct PrintSink;
impl ChatSink for PrintSink {
    fn on_text_delta(&self, delta: String) {
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

fn main() {
    eprintln!("creating PiSession...");
    let session = PiSession::new(Arc::new(PrintSink)).expect("PiSession::new failed");
    eprintln!("PiSession created, sending prompt");
    session.send("say hi in exactly 3 words".to_string());
    std::thread::sleep(Duration::from_secs(8));
    eprintln!("done");
}
