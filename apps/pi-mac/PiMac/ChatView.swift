import SwiftUI

/// Chat detail pane: composer, a `RowRecord`-rendered transcript (rich
/// markdown/code/tables, replacing SW1/SW2's plain-text-only display), plus
/// a richly-rendered, throttled live preview (`model.streamingRows`) for
/// whichever turn is still streaming — `AppModel.onHistoryReplaced` clears
/// it and hands back the finalized rows once the turn settles (see
/// `pi-core-ffi`'s `hydrate_and_push`).
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

    /// The composer's "current active model" picker (SW6) — checkmarks and
    /// bolds `isCurrent`, sorted by display label (name-first, so this
    /// reads as alphabetical-by-model-name) rather than pi's own
    /// `GetAvailableModels` order. Each row switches via `AppModel.
    /// setModel`. A `Menu` (not `Picker`) since per-row action closures
    /// need no separate `@State` selection binding kept in sync with
    /// `isCurrent`.
    private var modelPicker: some View {
        Menu {
            ForEach(sortedModels, id: \.id) { entry in
                Button {
                    Task { await model.setModel(provider: entry.provider, modelId: entry.id) }
                } label: {
                    if entry.isCurrent {
                        Label {
                            Text(entry.label).fontWeight(.semibold)
                        } icon: {
                            Image(systemName: "checkmark")
                        }
                    } else {
                        Text(entry.label)
                    }
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
            Text(model.isStreaming ? "streaming…" : "idle")
                .font(.caption)
                .foregroundStyle(.secondary)
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
