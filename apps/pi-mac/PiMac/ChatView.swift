import SwiftUI

/// Chat detail pane: composer, a `RowRecord`-rendered transcript (rich
/// markdown/code/tables, replacing SW1/SW2's plain-text-only display), a
/// richly-rendered, throttled live preview (`model.streamingRows`) for
/// whichever turn is still streaming, and (SW7) live thinking/tool-call
/// rows (`model.liveThinkingRows`/`liveToolRows`) — `AppModel.
/// onHistoryReplaced` clears all three and hands back the finalized rows
/// once the turn settles (see `pi-core-ffi`'s `hydrate_and_push`).
struct ChatView: View {
    var model: AppModel
    @State private var draft: String = ""

    var body: some View {
        VStack(spacing: 0) {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 10) {
                        ForEach(Array(model.rows.enumerated()), id: \.offset) { _, row in
                            RowView(row: row)
                        }
                        ForEach(model.liveThinkingRows) { block in
                            RowView(row: block.row)
                        }
                        ForEach(model.liveToolRows) { block in
                            RowView(row: block.row)
                        }
                        ForEach(Array(model.streamingRows.enumerated()), id: \.offset) { _, row in
                            RowView(row: row)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding()
                    .id("transcript-end")
                }
                .onChange(of: model.rows.count) {
                    proxy.scrollTo("transcript-end", anchor: .bottom)
                }
                .onChange(of: model.liveThinkingRows.count) {
                    proxy.scrollTo("transcript-end", anchor: .bottom)
                }
                .onChange(of: model.liveToolRows.count) {
                    proxy.scrollTo("transcript-end", anchor: .bottom)
                }
                .onChange(of: model.streamingRows.count) {
                    proxy.scrollTo("transcript-end", anchor: .bottom)
                }
            }

            Divider()

            statusBar

            HStack(alignment: .bottom, spacing: 8) {
                modelPicker

                TextField("Message pi…", text: $draft, axis: .vertical)
                    .textFieldStyle(.plain)
                    .lineLimit(1...6)
                    .padding(8)
                    .background(.quaternary, in: RoundedRectangle(cornerRadius: 8))
                    .onSubmit(send)

                if model.isStreaming {
                    Button("Abort", role: .destructive) {
                        model.abort()
                    }
                } else {
                    Button("Send") {
                        send()
                    }
                    .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
            .padding(12)
        }
        .frame(minWidth: 480, minHeight: 360)
        .navigationTitle(model.activeSessionPath == nil ? "New Session" : "pi")
        .extensionDialogs(model: model)
    }

    /// The composer's "current active model" picker (SW6) — checkmarks
    /// `isCurrent`, sorted by display label (name-first, so this reads as
    /// alphabetical-by-model-name) rather than pi's own `GetAvailableModels`
    /// order. Each row switches via `AppModel.setModel`. A `Menu` (not
    /// `Picker`) since per-row action closures need no separate `@State`
    /// selection binding kept in sync with `isCurrent`.
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
            Image(systemName: "cpu")
        }
        .menuIndicator(.hidden)
        .disabled(model.models.isEmpty)
        .help(model.models.first(where: \.isCurrent)?.label ?? "Choose a model")
    }

    private var sortedModels: [ModelRecord] {
        model.models.sorted {
            $0.label.localizedStandardCompare($1.label) == .orderedAscending
        }
    }

    private var statusBar: some View {
        HStack {
            Circle()
                .fill(model.isStreaming ? .green : .secondary)
                .frame(width: 8, height: 8)
                .help(model.isStreaming ? "Streaming" : "Idle")
            serverDotView
            Spacer()
            if let statusMessage = model.statusMessage {
                Text(statusMessage)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .lineLimit(1)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
    }

    /// Status-bar health dot for the active model's local server (SW6) —
    /// mirrors `app.slint`'s server-dot color semantics exactly (green/red/
    /// amber), hidden entirely for `.hidden` (cloud models, or no model
    /// resolved yet).
    @ViewBuilder
    private var serverDotView: some View {
        if let color = serverDotColor {
            Circle()
                .fill(color)
                .frame(width: 8, height: 8)
                .help(serverDotTooltip)
        }
    }

    private var serverDotColor: Color? {
        switch model.serverDot {
        case .hidden: return nil
        case .ok: return Color(red: 0x43 / 255, green: 0xa0 / 255, blue: 0x47 / 255)
        case .down: return Color(red: 0xd3 / 255, green: 0x54 / 255, blue: 0x54 / 255)
        case .mismatch: return Color(red: 0xe2 / 255, green: 0xa5 / 255, blue: 0x3f / 255)
        }
    }

    private var serverDotTooltip: String {
        switch model.serverDot {
        case .hidden: return ""
        case .ok: return "Local model server is healthy"
        case .down: return "Local model server is unreachable"
        case .mismatch: return "Local model server is running a different model than pi expects"
        }
    }

    private func send() {
        let prompt = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !prompt.isEmpty else { return }
        model.send(prompt)
        draft = ""
    }
}

#Preview {
    ChatView(model: AppModel())
}
