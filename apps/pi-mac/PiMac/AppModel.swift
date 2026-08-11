import Foundation
import Observation

/// Drives one `PiSession` (a spawned `pi --mode rpc` child, owned entirely
/// on the Rust side) plus the stateless `SessionIndex` browsing object, and
/// publishes both for `ChatView`/`SidebarView`.
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
final class AppModel: ChatSink {
    private(set) var transcript: String = ""
    private(set) var isStreaming: Bool = false
    private(set) var statusMessage: String?
    private(set) var activeSessionPath: String?

    private(set) var currentProject: String
    private(set) var projects: [ProjectRecord] = []
    private(set) var sessions: [SessionRecord] = []

    private var session: PiSession?
    private let sessionIndex = SessionIndex()

    init() {
        currentProject = Self.loadLastProject()
    }

    /// Spawns the `pi` child in `currentProject`, or — with `PI_MAC_DEMO` set
    /// in the environment — starts a synthetic session that streams a canned
    /// reply without `pi` installed (mirrors `SLINTY_DEMO=1` for the Slint
    /// app). Safe to call more than once; only the first call does anything.
    func start() {
        guard session == nil else { return }
        if ProcessInfo.processInfo.environment["PI_MAC_DEMO"] != nil {
            session = PiSession.newDemo(sink: self)
        } else {
            do {
                session = try PiSession(sink: self, cwd: currentProject)
            } catch {
                statusMessage = "Could not start pi: \(error)"
            }
        }
        Task { await refreshSidebar() }
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

    // MARK: - Sidebar: browsing (pull-based — re-fetched after every action)

    func refreshProjects() async {
        projects = await sessionIndex.listProjects()
    }

    func refreshSessions() async {
        sessions = await sessionIndex.listSessions(
            cwd: currentProject, query: "", activePath: activeSessionPath)
    }

    func refreshSidebar() async {
        await refreshProjects()
        await refreshSessions()
    }

    // MARK: - Sidebar: actions

    func switchProject(to path: String) async {
        guard let session else { return }
        do {
            try await session.switchProject(path: path)
            currentProject = path
            Self.saveLastProject(path)
            await refreshSidebar()
        } catch {
            statusMessage = "Could not switch project: \(error)"
        }
    }

    func startNewSession() async {
        guard let session else { return }
        do {
            try await session.newSession()
            await refreshSessions()
        } catch {
            statusMessage = "Could not start a new session: \(error)"
        }
    }

    /// `path` need not be the active session — matches `PiSession`'s own
    /// `delete_session`, which works on any listed path.
    func deleteSession(_ path: String) async {
        guard let session else { return }
        do {
            try await session.deleteSession(path: path)
            await refreshSessions()
        } catch {
            statusMessage = "Could not delete session: \(error)"
        }
    }

    /// Only the *active* session is renameable — `pi`'s `set_session_name`
    /// takes no path argument.
    func renameActiveSession(to name: String) async {
        guard let session else { return }
        do {
            try await session.renameSession(name: name)
            await refreshSessions()
        } catch {
            statusMessage = "Could not rename session: \(error)"
        }
    }

    // MARK: - Last-project persistence

    private static let lastProjectDefaultsKey = "dev.slinty-pi.pi-mac.lastProject"

    private static func loadLastProject() -> String {
        UserDefaults.standard.string(forKey: lastProjectDefaultsKey)
            ?? FileManager.default.homeDirectoryForCurrentUser.path
    }

    private static func saveLastProject(_ path: String) {
        UserDefaults.standard.set(path, forKey: lastProjectDefaultsKey)
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

    nonisolated func onActiveSessionChanged(path: String?) {
        Task { @MainActor in
            self.activeSessionPath = path
            self.transcript = ""
            await self.refreshSessions()
        }
    }
}
