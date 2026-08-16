import AppKit
import Foundation
import Observation
import os

/// One live thinking/tool-call row (SW7), keyed the same way Rust tracks it
/// (a synthetic per-region sequence for thinking, the real `tool_call_id`
/// for tool calls) so repeated pushes for the same id update in place.
struct LiveRow: Identifiable {
    let id: String
    var row: RowRecord
}

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
    /// Plain-text accumulator for whichever reply is still streaming — no
    /// longer rendered directly (see `streamingRows`); kept private since
    /// nothing outside this class needs to read it.
    private var transcript: String = ""
    /// Richly-rendered preview of `transcript`, refreshed on a throttled
    /// ~33ms cadence via `PiSession.previewRows` while a reply streams —
    /// closes the gap where streamed replies showed raw markdown until the
    /// turn settled. Cleared whenever `transcript` itself is (turn end,
    /// history replaced).
    private(set) var streamingRows: [RowRecord] = []
    /// Live thinking/tool-call rows (SW7) — pushed by Rust, not pulled like
    /// `streamingRows`: `RowView`'s existing `ThinkingRowView`/`ToolRowView`
    /// render these unchanged, they were already built against exactly this
    /// shape via the finalized `onHistoryReplaced` path. Rendered as two
    /// stable groups (all live thinking rows, then all live tool rows)
    /// rather than interleaved in exact chronological order with each other
    /// or with `streamingRows` — an explicit, simpler-on-purpose choice; see
    /// the SW7 plan's Design section. Cleared alongside `streamingRows`.
    private(set) var liveThinkingRows: [LiveRow] = []
    private(set) var liveToolRows: [LiveRow] = []
    private(set) var isStreaming: Bool = false
    private(set) var statusMessage: String?
    /// Path/sidebar-highlighting only — see `onActiveSessionChanged`'s doc
    /// comment in the generated `ChatSink` protocol.
    private(set) var activeSessionPath: String?

    private(set) var currentProject: String
    private(set) var projects: [ProjectRecord] = []
    private(set) var sessions: [SessionRecord] = []

    /// Transcript density (0 = Verbose, 1 = Normal, 2 = Summary) — mirrors
    /// `app.slint`'s `cycle-density()` semantics exactly (see the density
    /// control plan section). Persisted like `currentProject`, below.
    private(set) var density: Int

    /// `currentProject`'s last path component — the sole source both
    /// `SidebarView`'s column header and `ChatView`'s window-title bar read,
    /// so a project switch updates both at once.
    var projectDisplayName: String {
        (currentProject as NSString).lastPathComponent
    }

    // MARK: - Extension-UI dialogs (SW5)

    /// FIFO queue of unanswered `select`/`confirm`/`input`/`editor` dialog
    /// requests — pi's sanctioned tool-call permission-gating pattern (a
    /// `tool_call` extension + `confirm`/`select` dialog) surfaces through
    /// this same queue, not a separate one. `currentDialog` is the only one
    /// ever shown; `replyToCurrentDialog` pops it once answered.
    private(set) var pendingDialogs: [ExtensionDialogRecord] = []
    var currentDialog: ExtensionDialogRecord? { pendingDialogs.first }

    // MARK: - Models panel (SW4)

    private(set) var rapidMlxPanel: RapidMlxPanelRecord?
    private(set) var routerPanel: RouterPanelRecord?
    private(set) var ollamaPanel: OllamaPanelRecord?
    private(set) var authEntries: [String] = []
    private(set) var hfResults: [HfResultRecord] = []
    /// The last rapid-mlx action failure, rendered inline in the models
    /// panel. `statusMessage` isn't enough on its own: it only ever shows in
    /// the *chat* window, so a failure triggered from the Settings window
    /// would otherwise look like nothing happened at all.
    private(set) var rapidMlxError: String?

    // MARK: - Composer model picker + server dot (SW6)

    /// Pull-based, same convention as `sessions`/`rapidMlxPanel`: re-fetched
    /// after every action that could change model availability or
    /// selection, not pushed. `models.first(where: \.isCurrent)` is the
    /// composer picker's checkmarked entry.
    private(set) var models: [ModelRecord] = []
    /// The one piece of model state that *is* pushed (`onServerDotChanged`)
    /// — a 5-second-polled value with no natural user action to hang a
    /// re-fetch off of.
    private(set) var serverDot: ServerDotState = .hidden

    // MARK: - Thinking-level picker + session stats (SW8)

    /// Pull-based, mirroring `models` exactly: re-fetched after session
    /// start and after `setModel` (available levels differ per model).
    private(set) var thinkingLevels: [ThinkingLevelRecord] = []
    /// Pushed (`onSessionStatsChanged`) — no natural user action to hang a
    /// re-fetch off of, mirroring `serverDot`.
    private(set) var sessionStats: SessionStatsRecord?

    // MARK: - Attachments (SW9)

    /// Display names of images queued for the next prompt — pushed
    /// (`onPendingAttachmentsChanged`), the composer's attachment-chip row.
    private(set) var pendingAttachments: [String] = []
    /// A non-image `attachPath` call's `@path` reference, pushed
    /// (`onComposerAppend`) for `ChatView` to splice into its local
    /// `draft` — see `consumePendingComposerAppend`.
    private(set) var pendingComposerAppend: String?

    // MARK: - Session tree + fork-from (SW10)

    /// Flattened branch tree for the active session — pull-based, fetched
    /// when the tree sheet opens.
    private(set) var treeRows: [TreeRowRecord] = []
    /// A successful `forkFrom` call's forked-from prompt text, pushed
    /// (`onComposerReplace`) for `ChatView` to *replace* (not append to,
    /// unlike `pendingComposerAppend`) its local `draft` — see
    /// `consumePendingComposerReplace`.
    private(set) var pendingComposerReplace: String?

    private var session: PiSession?
    private let sessionIndex = SessionIndex()
    private let localModels = LocalModelIndex()
    /// Throttle state for `scheduleStreamPreview` — mirrors (not literally
    /// ports) `pi_core::backend::Transcript::flush_stream`'s `TEXT_FLUSH`
    /// (33ms) gating: a burst of deltas within the window coalesces into
    /// one `previewRows` call once it elapses, rather than one call per
    /// delta.
    private static let previewFlushInterval: Duration = .milliseconds(33)
    private var lastPreviewFlush: ContinuousClock.Instant?
    private var pendingPreviewTask: Task<Void, Never>?
    /// Reset on every new turn (`onStreamingChanged(streaming: true)`), set
    /// by `onError` — read once the turn settles so the "turn completed"
    /// notification's title can distinguish success from error, since
    /// `onStreamingChanged(streaming: false)` itself carries no outcome.
    private var turnHadError = false
    /// Same subsystem the Rust side's `tracing-oslog` subscriber uses (see
    /// `pi-core-ffi`'s `ensure_logging_initialized`) — so every message
    /// that's ever shown to the user, from either layer, ends up in the
    /// same place in Console.app / `log stream`, not just a one-line status
    /// caption that gets overwritten by the next event.
    private let logger = Logger(subsystem: "dev.slinty-pi.swifty-pi", category: "app")

    init() {
        currentProject = Self.loadLastProject()
        density = Self.loadDensity()
    }

    /// Sets density directly (0 = Verbose, 1 = Normal, 2 = Summary) — the
    /// composer toolbar's density `Picker` writes through this.
    func setDensity(_ value: Int) {
        density = value
        Self.saveDensity(density)
    }

    /// Whether a row of `kind` should render at the given `density` — a
    /// pure mirror of `app.slint`'s `Style.row-visible`: errors always
    /// show; Summary (2) hides `thinking`/`tool`/`info` rows (live ones
    /// included, not just finalized); Verbose/Normal (0/1) show everything.
    static func rowVisible(density: Int, kind: String) -> Bool {
        if kind == "error" { return true }
        guard density == 2 else { return true }
        return !["thinking", "tool", "info"].contains(kind)
    }

    /// Sets `statusMessage` and logs it — the only way `statusMessage`
    /// should ever be assigned (see the doc comment on `logger`).
    private func setStatus(_ message: String) {
        logger.error("\(message)")
        statusMessage = message
    }

    /// Spawns the `pi` child in `currentProject`, or — with `SWIFTY_PI_DEMO` set
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
        if ProcessInfo.processInfo.environment["SWIFTY_PI_DEMO"] != nil {
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
        NotificationController.shared.requestAuthorizationIfNeeded()
        Task { await refreshSidebar() }
        Task { await refreshModels() }
        Task { await refreshThinkingLevels() }
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

    // MARK: - Attachments (SW9)

    /// An image is queued (see `onPendingAttachmentsChanged`); anything
    /// else pushes an `@path` reference (see `onComposerAppend`). Plain,
    /// fire-and-forget — mirrors `PiSession.attachPath`'s own shape.
    func attachPath(_ path: String) {
        session?.attachPath(path: path)
    }

    /// Queues raw image bytes (pasted from the clipboard, no backing file)
    /// as an attachment — the paste counterpart to `attachPath`'s image
    /// branch, for data that never had a path to read from disk.
    func attachImageData(name: String, mimeType: String, data: Data) {
        session?.attachImageData(name: name, mimeType: mimeType, data: data)
    }

    /// Removes a queued image by its index in the chip row.
    func removeAttachment(at index: Int) {
        session?.removeAttachment(index: UInt32(index))
    }

    /// Reads and clears `pendingComposerAppend` in one call — avoids a
    /// race between `ChatView` reading the value and clearing it across
    /// separate steps.
    func consumePendingComposerAppend() -> String? {
        defer { pendingComposerAppend = nil }
        return pendingComposerAppend
    }

    // MARK: - Session tree + fork-from (SW10)

    /// Fetches and flattens the active session's branch tree. Pull-based —
    /// call when the tree sheet opens.
    func openTree() async {
        guard let session else { return }
        do {
            treeRows = try await session.openTree()
        } catch {
            setStatus("Could not load session tree: \(error)")
        }
    }

    /// Rewinds the active branch to before `entryId`. On success, the
    /// transcript re-hydrates and `onComposerReplace` fires with the
    /// forked-from prompt text — no manual refetch needed here, mirroring
    /// how SW9's attach flow already works.
    func forkFrom(entryId: String) async {
        guard let session else { return }
        do {
            try await session.forkFrom(entryId: entryId)
        } catch {
            setStatus("Could not fork: \(error)")
        }
    }

    /// Reads and clears `pendingComposerReplace` in one call — same
    /// atomic shape as `consumePendingComposerAppend`.
    func consumePendingComposerReplace() -> String? {
        defer { pendingComposerReplace = nil }
        return pendingComposerReplace
    }

    /// Also cancels any pending extension dialog(s) — a defensive,
    /// client-side safety net, since whether pi itself cancels a gated
    /// tool call's dialog on abort is unconfirmed (see the SW5 plan's
    /// Risks). Without this, aborting mid-dialog could otherwise leave the
    /// gated extension hanging until its own `timeout` elapses.
    func abort() {
        replyToCurrentDialog(.cancelled)
        pendingDialogs.removeAll()
        session?.abort()
    }

    /// Pops and answers `currentDialog`, if any — a no-op if the queue is
    /// already empty (safe to call from `abort()` unconditionally).
    func replyToCurrentDialog(_ reply: ExtensionDialogReply) {
        guard let dialog = pendingDialogs.first else { return }
        pendingDialogs.removeFirst()
        session?.replyExtensionDialog(requestId: dialog.id, reply: reply)
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

    /// Native folder picker → `switchProject` — shared by the sidebar's
    /// "Switch Project" toolbar button and the File menu's "Open Project…"
    /// command, so both trigger the exact same panel/switch sequence.
    func promptSwitchProject() async {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = "Switch"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        await switchProject(to: url.path)
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
        // rapid-mlx comes from `PiSession`, not `LocalModelIndex`: resolving
        // each row's serve/stop/register state needs the live client (which
        // models pi knows) and the managed child (which server is ours).
        async let router = localModels.refreshRouterPanel()
        async let ollama = localModels.refreshOllamaPanel()
        async let auth = localModels.refreshAuthEntries()
        await refreshRapidMlxPanel()
        routerPanel = await router
        ollamaPanel = await ollama
        authEntries = await auth
    }

    func refreshRapidMlxPanel() async {
        guard let session else { return }
        do {
            rapidMlxPanel = try await session.refreshRapidMlxPanel()
        } catch {
            setStatus("Could not read rapid-mlx state: \(error)")
        }
    }

    // MARK: - Models panel: actions

    /// Starts a rapid-mlx server for `alias`, replacing any this app already
    /// runs. Doesn't change pi's selected model — serving and selecting are
    /// separate steps.
    func serveRapidMlx(alias: String) async {
        await runRapidMlxAction("serve \(alias)") { session in
            try await session.serveRapidMlxModel(alias: alias)
        }
    }

    /// Stops the rapid-mlx server this app started. Fails with an
    /// explanation when the running server isn't ours.
    func stopRapidMlx() async {
        await runRapidMlxAction("stop rapid-mlx") { session in
            try await session.stopRapidMlx()
        }
    }

    /// Makes `alias` selectable by pi (writes models.json, refreshes pi's
    /// catalog, restarts the child). The row becomes servable afterward.
    func registerRapidMlx(alias: String) async {
        await runRapidMlxAction("register \(alias)") { session in
            try await session.registerRapidMlxModel(alias: alias)
        }
    }

    /// Shared shape for the three rapid-mlx actions: clear the last error,
    /// run, then refresh.
    ///
    /// The refresh happens twice — immediately, and again shortly after —
    /// because a just-spawned server takes a moment to show up in
    /// `rapid-mlx ps`. Two bounded refreshes rather than a polling loop:
    /// enough for the panel to settle on its own without the window being
    /// closed and reopened.
    private func runRapidMlxAction(
        _ what: String,
        _ body: @escaping (PiSession) async throws -> Void
    ) async {
        guard let session else { return }
        rapidMlxError = nil
        do {
            try await body(session)
        } catch {
            let message = "Could not \(what): \(error)"
            rapidMlxError = message
            setStatus(message)
        }
        await refreshModelsPanel()
        await refreshModels()
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(2))
            await self?.refreshRapidMlxPanel()
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
        await refreshModels()
    }

    func unloadRouterModel(id: String) async {
        do {
            try await localModels.startUnloadRouterModel(id: id)
        } catch {
            setStatus("Could not unload \(id): \(error)")
        }
        await pollRouterUntilIdle()
        await refreshModels()
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
        await refreshModels()
    }

    // MARK: - Composer model picker: browsing + actions

    /// Pull, mirroring `refreshModelsPanel`'s shape: `GetAvailableModels` +
    /// `GetState` re-fetched fresh every call (see `pi-core-ffi`'s
    /// `refresh_models_and_state`), not incrementally patched.
    func refreshModels() async {
        guard let session else { return }
        do {
            models = try await session.refreshModels()
        } catch {
            setStatus("Could not list models: \(error)")
        }
    }

    /// Switches pi's active model, then refreshes so the new selection's
    /// checkmark is correct — action-then-refetch, this app's established
    /// pattern (e.g. `serveRapidMlx`).
    func setModel(provider: String, modelId: String) async {
        guard let session else { return }
        do {
            try await session.setModel(provider: provider, modelId: modelId)
            await refreshModels()
            // Available thinking levels differ per model — mirrors
            // pi_core::backend's own SetModel -> refresh_thinking sequence.
            await refreshThinkingLevels()
        } catch {
            setStatus("Could not switch model: \(error)")
        }
    }

    // MARK: - Thinking-level picker: browsing + actions

    /// Pull, mirroring `refreshModels`'s shape exactly.
    func refreshThinkingLevels() async {
        guard let session else { return }
        do {
            thinkingLevels = try await session.refreshThinkingLevels()
        } catch {
            setStatus("Could not list thinking levels: \(error)")
        }
    }

    /// Switches pi's active thinking level, then refreshes so the new
    /// selection's checkmark is correct — mirrors `setModel`.
    func setThinkingLevel(_ level: ThinkingLevelKind) async {
        guard let session else { return }
        do {
            try await session.setThinkingLevel(level: level)
            await refreshThinkingLevels()
        } catch {
            setStatus("Could not switch thinking level: \(error)")
        }
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

    private static let lastProjectDefaultsKey = "dev.slinty-pi.swifty-pi.lastProject"

    private static func loadLastProject() -> String {
        UserDefaults.standard.string(forKey: lastProjectDefaultsKey)
            ?? FileManager.default.homeDirectoryForCurrentUser.path
    }

    private static func saveLastProject(_ path: String) {
        UserDefaults.standard.set(path, forKey: lastProjectDefaultsKey)
    }

    // MARK: - Last-session-per-project persistence (launch-time restore)

    private static let lastSessionsDefaultsKey = "dev.slinty-pi.swifty-pi.lastSessionByProject"

    private static func loadLastSession(forProject project: String) -> String? {
        let byProject = UserDefaults.standard.dictionary(forKey: lastSessionsDefaultsKey) as? [String: String]
        return byProject?[project]
    }

    private static func saveLastSession(_ path: String, forProject project: String) {
        var byProject = UserDefaults.standard.dictionary(forKey: lastSessionsDefaultsKey) as? [String: String] ?? [:]
        byProject[project] = path
        UserDefaults.standard.set(byProject, forKey: lastSessionsDefaultsKey)
    }

    // MARK: - Density persistence

    private static let densityDefaultsKey = "dev.slinty-pi.swifty-pi.density"

    private static func loadDensity() -> Int {
        (UserDefaults.standard.object(forKey: densityDefaultsKey) as? Int) ?? 1 // Normal
    }

    private static func saveDensity(_ value: Int) {
        UserDefaults.standard.set(value, forKey: densityDefaultsKey)
    }

    // MARK: - ChatSink

    nonisolated func onTextDelta(delta: String) {
        Task { @MainActor in
            self.transcript += delta
            self.scheduleStreamPreview()
        }
    }

    nonisolated func onTurnEnd() {
        Task { @MainActor in
            self.transcript += "\n"
            self.pendingPreviewTask?.cancel()
            self.pendingPreviewTask = nil
            self.streamingRows = []
            self.liveThinkingRows = []
            self.liveToolRows = []
        }
    }

    nonisolated func onStreamingChanged(streaming: Bool) {
        Task { @MainActor in
            self.isStreaming = streaming
            if streaming {
                self.turnHadError = false
            } else {
                NotificationController.shared.notifyTurnCompleted(
                    project: self.projectDisplayName, hadError: self.turnHadError)
            }
        }
    }

    nonisolated func onError(message: String) {
        Task { @MainActor in
            self.setStatus(message)
            self.turnHadError = true
        }
    }

    nonisolated func onActiveSessionChanged(path: String?) {
        Task { @MainActor in
            self.activeSessionPath = path
            if let path {
                Self.saveLastSession(path, forProject: self.currentProject)
            }
            // No reply sent — the request's originating client/session
            // context is presumed moot once it changes (see the SW5 plan's
            // Risks for the one known edge case: same-client SwitchSession).
            self.pendingDialogs.removeAll()
            await self.refreshSessions()
        }
    }

    nonisolated func onHistoryReplaced(rows: [RowRecord]) {
        Task { @MainActor in
            SpeechController.shared.stop()
            self.rows = rows
            self.transcript = ""
            self.pendingPreviewTask?.cancel()
            self.pendingPreviewTask = nil
            self.streamingRows = []
            self.liveThinkingRows = []
            self.liveToolRows = []
            self.pendingDialogs.removeAll()
        }
    }

    nonisolated func onExtensionDialog(request: ExtensionDialogRecord) {
        Task { @MainActor in
            self.pendingDialogs.append(request)
            NotificationController.shared.notifyDialogPending(request)
        }
    }

    nonisolated func onServerDotChanged(state: ServerDotState) {
        Task { @MainActor in
            self.serverDot = state
        }
    }

    nonisolated func onSessionStatsChanged(stats: SessionStatsRecord) {
        Task { @MainActor in
            self.sessionStats = stats
        }
    }

    nonisolated func onPendingAttachmentsChanged(names: [String]) {
        Task { @MainActor in
            self.pendingAttachments = names
        }
    }

    nonisolated func onComposerAppend(text: String) {
        Task { @MainActor in
            self.pendingComposerAppend = text
        }
    }

    nonisolated func onComposerReplace(text: String) {
        Task { @MainActor in
            self.pendingComposerReplace = text
        }
    }

    nonisolated func onThinkingRowChanged(id: String, row: RowRecord) {
        Task { @MainActor in
            Self.upsert(LiveRow(id: id, row: row), into: &self.liveThinkingRows)
        }
    }

    nonisolated func onToolRowChanged(id: String, row: RowRecord) {
        Task { @MainActor in
            Self.upsert(LiveRow(id: id, row: row), into: &self.liveToolRows)
        }
    }

    /// Replaces the entry matching `row.id` in place if one exists
    /// (preserving its position), else appends — first-seen order becomes
    /// display order for `liveThinkingRows`/`liveToolRows`.
    private static func upsert(_ row: LiveRow, into rows: inout [LiveRow]) {
        if let index = rows.firstIndex(where: { $0.id == row.id }) {
            rows[index] = row
        } else {
            rows.append(row)
        }
    }

    // MARK: - Live streaming preview (throttled `previewRows` calls)

    /// Coalesces bursts of `onTextDelta` calls into one `previewRows` call
    /// per `previewFlushInterval`, the same shape `flush_stream`'s
    /// `last_flush.elapsed() >= TEXT_FLUSH` gate produces on the Rust side:
    /// a delta arriving right after a flush waits out the rest of the
    /// window; a delta arriving after a long gap flushes immediately.
    /// Only one preview refresh is ever scheduled/in-flight at a time —
    /// a second call while one is already pending is a no-op.
    private func scheduleStreamPreview() {
        guard pendingPreviewTask == nil else { return }
        let elapsed = lastPreviewFlush?.duration(to: .now) ?? Self.previewFlushInterval
        let delay = max(.zero, Self.previewFlushInterval - elapsed)
        pendingPreviewTask = Task { [weak self] in
            if delay > .zero {
                try? await Task.sleep(for: delay)
            }
            guard let self, !Task.isCancelled else { return }
            await self.refreshStreamPreview()
        }
    }

    private func refreshStreamPreview() async {
        guard let session else { return }
        pendingPreviewTask = nil
        lastPreviewFlush = .now
        streamingRows = await session.previewRows(markdown: transcript)
    }
}
