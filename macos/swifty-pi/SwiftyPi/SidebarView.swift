import SwiftUI

/// Session browsing + lifecycle actions: list, switch project, new, delete
/// (behind a confirmation alert — the underlying delete is Trash-based/
/// recoverable, but a context-menu click alone was one accidental tap away),
/// rename, and — as of SW3 — click a non-active row to resume it (loads its
/// history via `AppModel.switchSession`/`ChatSink.onHistoryReplaced`).
struct SidebarView: View {
    var model: AppModel

    @State private var isRenaming = false
    @State private var renameText = ""
    @State private var showModels = false
    @State private var sessionPendingDelete: SessionRecord?

    var body: some View {
        List(model.sessions, id: \.path) { session in
            SessionRowView(session: session)
                .contentShape(Rectangle())
                .onTapGesture {
                    guard !session.active else { return }
                    Task { await model.switchSession(to: session.path) }
                }
                .contextMenu {
                    if session.active {
                        Button("Rename…") {
                            renameText = session.title
                            isRenaming = true
                        }
                    }
                    Button("Delete", role: .destructive) {
                        sessionPendingDelete = session
                    }
                }
        }
        .listStyle(.sidebar)
        .navigationTitle(model.projectDisplayName)
        .safeAreaInset(edge: .bottom) {
            VStack(spacing: 0) {
                Divider()
                HStack(spacing: 8) {
                    modelPicker
                    // Hidden entirely below 2 levels, matching app.slint's
                    // `thinking-list.length > 1` gate — most models offer
                    // just one (or no) thinking level, so this stays out of
                    // the way for those; modelPicker alone then fills the row.
                    if model.thinkingLevels.count > 1 {
                        thinkingLevelPicker
                    }
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            }
        }
        .toolbar {
            ToolbarItem {
                Button {
                    Task { await model.startNewSession() }
                } label: {
                    Label("New Session", systemImage: "square.and.pencil")
                }
                .help("Start a new session in \(model.projectDisplayName)")
            }
            ToolbarItem {
                Button {
                    Task { await model.promptSwitchProject() }
                } label: {
                    Label("Switch Project", systemImage: "folder")
                }
                .help("Switch to a different project directory")
            }
            ToolbarItem {
                Button {
                    showModels = true
                } label: {
                    Label("Models", systemImage: "cpu")
                }
                .help("Browse and manage local models")
                .keyboardShortcut("m")
            }
        }
        .alert("Rename Session", isPresented: $isRenaming) {
            TextField("Name", text: $renameText)
            Button("Cancel", role: .cancel) {}
            Button("Rename") {
                Task { await model.renameActiveSession(to: renameText) }
            }
        }
        .sheet(isPresented: $showModels) {
            ModelsPanelView(model: model)
        }
        .alert(
            "Delete Session?",
            isPresented: Binding(
                get: { sessionPendingDelete != nil },
                set: { presented in if !presented { sessionPendingDelete = nil } }
            ),
            presenting: sessionPendingDelete
        ) { session in
            Button("Cancel", role: .cancel) {}
            Button("Delete", role: .destructive) {
                Task { await model.deleteSession(session.path) }
            }
        } message: { session in
            Text("\"\(session.title)\" will be moved to the Trash. You can recover it from there if needed.")
        }
    }

    /// The "current active model" picker (SW6) — checkmarks `isCurrent`,
    /// sorted by display label (name-first, so this reads as
    /// alphabetical-by-model-name) rather than pi's own `GetAvailableModels`
    /// order. Each row switches via `AppModel.setModel`. A `Menu` (not
    /// `Picker`) since per-row action closures need no separate `@State`
    /// selection binding kept in sync with `isCurrent`. Lives at the bottom
    /// of the sidebar (via `.safeAreaInset`), so its trigger shows the
    /// current model's name rather than a bare icon — unlike a composer
    /// chip, there's no adjacent context to lean on here.
    ///
    /// Keyed by array offset, not `entry.id` — pi can legitimately list the
    /// same model `id` under more than one provider (e.g. the same model
    /// proxied through both a direct provider and a router like `bifrost`),
    /// so `id` alone isn't a unique `ForEach` identity and produced
    /// duplicated/misplaced rows when used as one.
    ///
    /// The current row's checkmark is a plain text prefix, not `Label(_:
    /// systemImage:)` — an SF Symbol icon inside a `Menu` button label
    /// wasn't rendering reliably (observed on a beta macOS/Xcode SDK), and
    /// plain `Text` has no such ambiguity.
    private var modelPicker: some View {
        Menu {
            ForEach(Array(sortedModels.enumerated()), id: \.offset) { _, entry in
                Button {
                    Task { await model.setModel(provider: entry.provider, modelId: entry.id) }
                } label: {
                    Text(entry.isCurrent ? "✓ \(entry.label)" : entry.label)
                }
            }
        } label: {
            HStack {
                Image(systemName: "cpu")
                Text(currentModelLabel)
                    .lineLimit(1)
                Spacer(minLength: 0)
            }
        }
        .menuIndicator(.hidden)
        .disabled(model.models.isEmpty)
        .help(currentModelLabel)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var currentModelLabel: String {
        model.models.first(where: \.isCurrent)?.label ?? "Choose a model"
    }

    private var sortedModels: [ModelRecord] {
        model.models.sorted {
            $0.label.localizedStandardCompare($1.label) == .orderedAscending
        }
    }

    /// The active thinking-level picker (SW8) — same `Menu`/checkmark-
    /// prefix/offset-keyed shape as `modelPicker` (see its doc comment for
    /// the reasoning), just for the model's currently-available thinking
    /// levels instead of the model list itself.
    private var thinkingLevelPicker: some View {
        Menu {
            ForEach(Array(model.thinkingLevels.enumerated()), id: \.offset) { _, entry in
                Button {
                    Task { await model.setThinkingLevel(entry.level) }
                } label: {
                    Text(entry.isCurrent ? "✓ \(entry.label)" : entry.label)
                }
            }
        } label: {
            HStack {
                Image(systemName: "brain")
                Text(currentThinkingLabel)
                    .lineLimit(1)
                Spacer(minLength: 0)
            }
        }
        .menuIndicator(.hidden)
        .help(currentThinkingLabel)
        .fixedSize()
    }

    private var currentThinkingLabel: String {
        model.thinkingLevels.first(where: \.isCurrent)?.label ?? "Choose thinking level"
    }
}

private struct SessionRowView: View {
    let session: SessionRecord

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text(session.title)
                    .fontWeight(session.active ? .semibold : .regular)
                    .lineLimit(1)
                Spacer()
                if !session.cost.isEmpty {
                    Text(session.cost)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            Text(session.relativeTime)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .opacity(session.active ? 1.0 : 0.6)
        .padding(.vertical, 2)
    }
}

#Preview {
    SidebarView(model: AppModel())
}
