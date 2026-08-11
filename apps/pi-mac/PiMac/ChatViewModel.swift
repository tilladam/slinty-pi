import Foundation
import Observation

/// Drives one `PiSession` (a spawned `pi --mode rpc` child, owned entirely
/// on the Rust side) and publishes its state for `ChatView`.
///
/// Conforms to the generated `ChatSink` protocol so Rust can call back into
/// it directly — those methods run on a Rust tokio worker thread, not the
/// main actor, so each is `nonisolated` and hops to `@MainActor` itself
/// before touching published state. This is the same responsibility
/// `Weak::upgrade_in_event_loop` discharges on the Slint side of this
/// project (see `pi_core::backend::UiSink`'s doc comment) — just relocated
/// to the Swift side of the FFI boundary.
@Observable
@MainActor
final class ChatViewModel: ChatSink {
    private(set) var transcript: String = ""
    private(set) var isStreaming: Bool = false
    private(set) var statusMessage: String?

    private var session: PiSession?

    /// Spawns the `pi` child. Safe to call more than once; only the first
    /// call does anything.
    func start() {
        guard session == nil else { return }
        do {
            session = try PiSession(sink: self)
        } catch {
            statusMessage = "Could not start pi: \(error)"
        }
    }

    func send(_ prompt: String) {
        guard let session else {
            statusMessage = "pi hasn't started yet"
            return
        }
        transcript += transcript.isEmpty ? "> \(prompt)\n\n" : "\n\n> \(prompt)\n\n"
        session.send(prompt: prompt)
    }

    func abort() {
        session?.abort()
    }

    // MARK: - ChatSink

    nonisolated func onTextDelta(delta: String) {
        Task { @MainActor in
            self.transcript += delta
        }
    }

    nonisolated func onTurnEnd() {
        Task { @MainActor in
            self.transcript += "\n"
        }
    }

    nonisolated func onStreamingChanged(streaming: Bool) {
        Task { @MainActor in
            self.isStreaming = streaming
        }
    }

    nonisolated func onError(message: String) {
        Task { @MainActor in
            self.statusMessage = message
        }
    }
}
