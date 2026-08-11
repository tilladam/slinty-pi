import Foundation
import Observation
import os

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
    /// Richly-rendered rows for everything up to the currently-streaming
    /// turn — the single source of truth for transcript content as of SW3
    /// (`onHistoryReplaced`, not `onActiveSessionChanged`, owns this).
    private(set) var rows: [RowRecord] = []
    /// Plain-text bubble for whichever reply is still streaming; cleared
    /// the moment `onHistoryReplaced` delivers the finalized, richly-
    /// rendered rows for that same turn.
    private(set) var transcript: String = ""
    private(set) var isStreaming: Bool = false
    private(set) var statusMessage: String?
    /// Path/sidebar-highlighting only — see `onActiveSessionChanged`'s doc
    /// comment in the generated `ChatSink` protocol.
    private(set) var activeSessionPath: String?

    private(set) var currentProject: String
    private(set) var projects: [ProjectRecord] = []
    private(set) var sessions: [SessionRecord] = []

    // MARK: - Models panel (SW4)

    private(set) var rapidMlxPanel: RapidMlxPanelRecord?
    private(set) var routerPanel: RouterPanelRecord?
    private(set) var ollamaPanel: OllamaPanelRecord?
    private(set) var authEntries: [String] = []
    private(set) var hfResults: [HfResultRecord] = []

    private var session: PiSession?
    private let sessionIndex = SessionIndex()
    private let localModels = LocalModelIndex()
    /// Same subsystem the Rust side's `tracing-oslog` subscriber uses (see
    /// `pi-core-ffi`'s `ensure_logging_initialized`) — so every message
    /// that's ever shown to the user, from either layer, ends up in the
    /// same place in Console.app / `log stream`, not just a one-line status
    /// caption that gets overwritten by the next event.
    private let logger = Logger(subsystem: "dev.slinty-pi.pi-mac", category: "app")

    init() {
        currentProject = Self.loadLastProject()
    }

    /// Sets `statusMessage` and logs it — the only way `statusMessage`
    /// should ever be assigned (see the doc comment on `logger`).
    private func setStatus(_ message: String) {
        logger.error("\(message)")
        statusMessage = message
    }

    /// Spawns the `pi` child in `currentProject`, or — with `PI_MAC_DEMO` set
    /// in the environment — starts a synthetic session that streams a canned
    /// reply without `pi` installed (mirrors `SLINTY_DEMO=1` for the Slint
    /// app). Safe to call more than once; only the first call does anything.
    /// `dark` seeds the syntax-highlighting theme `PiSession` hydrates
    /// against — `ContentView` reads it from `\.colorScheme` since this
    /// class isn't itself a `View` and has no `@Environment` of its own.
    /// Resumes `currentProject`'s last-active session automatically, if one
    /// is known — launch-time "restore," mirroring `pi_backend`'s
    /// `resume_on_first_spawn`.
    func start(dark: Bool) {
        guard session == nil else { return }
        if ProcessInfo.processInfo.environment["PI_MAC_DEMO"] != nil {
            session = PiSession.newDemo(sink: self)
        } else {
            do {
                session = try PiSession(
                    sink: self,
                    cwd: currentProject,
                    resumeSessionPath: Self.loadLastSession(forProject: currentProject),
                    dark: dark
                )
            } catch {
                setStatus("Could not start pi: \(error)")
            }
        }
        Task { await refreshSidebar() }
    }

    /// Called once at startup and again on every `colorScheme` change.
    func setDarkMode(_ dark: Bool) {
        session?.setDarkMode(dark: dark)
    }

    func send(_ prompt: String) {
        guard let session else {
            setStatus("pi hasn't started yet")
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
            setStatus("Could not switch project: \(error)")
        }
    }

    func startNewSession() async {
        guard let session else { return }
        do {
            try await session.newSession()
            await refreshSessions()
        } catch {
            setStatus("Could not start a new session: \(error)")
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
            setStatus("Could not delete session: \(error)")
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
            setStatus("Could not rename session: \(error)")
        }
    }

    /// Loads `path`'s history via `onHistoryReplaced` — the sidebar's
    /// click-to-resume action. `path`'s persistence as "last active session
    /// for this project" happens in `onActiveSessionChanged`, not here, so
    /// every path to becoming active (an explicit switch, or the first
    /// message in a brand-new session) is captured the same way.
    func switchSession(to path: String) async {
        guard let session else { return }
        do {
            try await session.switchSession(path: path)
        } catch {
            setStatus("Could not switch session: \(error)")
        }
    }

    // MARK: - Models panel: browsing (pull-based, same shape as the sidebar)

    /// Refreshes all four sections concurrently. Kept as one call for the
    /// panel's "open" moment; `loadRouterModel`/`unloadRouterModel`/
    /// `downloadHfModel`'s own polling only ever re-fetches the router
    /// section on its own — collecting rapid-mlx state shells out to the
    /// CLI a handful of times, which a router-only poll tick must not
    /// repeat (see `LocalModelIndex`'s doc comment).
    func refreshModelsPanel() async {
        async let rapidMlx = localModels.refreshRapidMlxPanel()
        async let router = localModels.refreshRouterPanel()
        async let ollama = localModels.refreshOllamaPanel()
        async let auth = localModels.refreshAuthEntries()
        rapidMlxPanel = await rapidMlx
        routerPanel = await router
        ollamaPanel = await ollama
        authEntries = await auth
    }

    // MARK: - Models panel: actions

    /// (Re)spawns a managed rapid-mlx server and makes it pi's active model
    /// — the one local-model action that goes through `PiSession`, not
    /// `LocalModelIndex` (see `PiSession.serveRapidMlxModel`'s doc comment).
    func serveRapidMlx(alias: String) async {
        guard let session else { return }
        do {
            try await session.serveRapidMlxModel(alias: alias)
            await refreshModelsPanel()
        } catch {
            setStatus("Could not serve \(alias): \(error)")
        }
    }

    func loadRouterModel(id: String) async {
        do {
            try await localModels.startLoadRouterModel(id: id)
        } catch {
            setStatus("Could not load \(id): \(error)")
            return
        }
        await pollRouterUntilIdle()
    }

    func unloadRouterModel(id: String) async {
        do {
            try await localModels.startUnloadRouterModel(id: id)
        } catch {
            setStatus("Could not unload \(id): \(error)")
        }
        await pollRouterUntilIdle()
    }

    /// `model` is `"owner/repo:quant"`, as built by `HfSearchView`'s quant
    /// chips.
    func downloadHfModel(_ model: String) async {
        do {
            try await localModels.startDownloadRouterModel(model: model)
        } catch {
            setStatus("Could not start download of \(model): \(error)")
            return
        }
        await pollRouterUntilIdle()
    }

    func searchHfModels(query: String) async {
        do {
            hfResults = try await localModels.searchHfModels(query: query)
        } catch {
            setStatus("Hugging Face search failed: \(error)")
            hfResults = []
        }
    }

    func addOllamaToPi() async {
        do {
            try await localModels.addOllamaToPi()
        } catch {
            setStatus("Could not add Ollama models: \(error)")
        }
    }

    func saveApiKey(provider: String, key: String) async {
        do {
            try await localModels.saveApiKey(provider: provider, key: key)
            authEntries = await localModels.refreshAuthEntries()
        } catch {
            setStatus("Could not save API key: \(error)")
        }
    }

    /// Polls the router section every 500ms until nothing is loading/
    /// downloading, or 120s elapse — the Swift-side counterpart to
    /// `pi_core::backend::poll_router_until_idle`, deliberately not ported
    /// as a blocking Rust call (see `LocalModelIndex.startLoadRouterModel`'s
    /// doc comment).
    private func pollRouterUntilIdle() async {
        let deadline = Date().addingTimeInterval(120)
        while true {
            routerPanel = await localModels.refreshRouterPanel()
            let busy = routerPanel?.models.contains(where: { $0.busy }) ?? false
            guard busy, Date() < deadline else { return }
            try? await Task.sleep(for: .milliseconds(500))
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

    // MARK: - Last-session-per-project persistence (launch-time restore)

    private static let lastSessionsDefaultsKey = "dev.slinty-pi.pi-mac.lastSessionByProject"

    private static func loadLastSession(forProject project: String) -> String? {
        let byProject = UserDefaults.standard.dictionary(forKey: lastSessionsDefaultsKey) as? [String: String]
        return byProject?[project]
    }

    private static func saveLastSession(_ path: String, forProject project: String) {
        var byProject = UserDefaults.standard.dictionary(forKey: lastSessionsDefaultsKey) as? [String: String] ?? [:]
        byProject[project] = path
        UserDefaults.standard.set(byProject, forKey: lastSessionsDefaultsKey)
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
            self.setStatus(message)
        }
    }

    nonisolated func onActiveSessionChanged(path: String?) {
        Task { @MainActor in
            self.activeSessionPath = path
            if let path {
                Self.saveLastSession(path, forProject: self.currentProject)
            }
            await self.refreshSessions()
        }
    }

    nonisolated func onHistoryReplaced(rows: [RowRecord]) {
        Task { @MainActor in
            self.rows = rows
            self.transcript = ""
        }
    }
}
