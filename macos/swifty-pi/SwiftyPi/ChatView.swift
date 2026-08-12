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
                    Button {
                        send()
                    } label: {
                        Label("Send", systemImage: "paperplane.fill")
                    }
                    .labelStyle(.iconOnly)
                    .help("Send")
                    .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
            .padding(12)
        }
        .frame(minWidth: 480, minHeight: 360)
        .navigationTitle(model.activeSessionPath == nil ? "New Session" : "pi")
        .toolbar {
            // Streaming + server-health indicator dots, trailing edge of the
            // top navigation bar — moved here from the composer's own status
            // bar so they're visible regardless of scroll position.
            // `.sharedBackgroundVisibility(.hidden)` (macOS 26+ only — the
            // app's deployment target is 14+) opts this item out of the
            // system's automatic grouped "glass" background: these are
            // plain status dots, not a control, so they shouldn't look like
            // a button. Older macOS versions don't have that background to
            // begin with, so the plain `ToolbarItem` fallback needs no
            // equivalent workaround.
            if #available(macOS 26.0, *) {
                ToolbarItem(placement: .primaryAction) {
                    statusDots
                }
                .sharedBackgroundVisibility(.hidden)
            } else {
                ToolbarItem(placement: .primaryAction) {
                    statusDots
                }
            }
        }
        .extensionDialogs(model: model)
    }

    private var statusDots: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(model.isStreaming ? .green : .secondary)
                .frame(width: 12, height: 12)
                .padding(4)
                .contentShape(Rectangle())
                .help(model.isStreaming ? "Streaming" : "Idle")
            serverDotView
        }
    }

    private var statusBar: some View {
        HStack {
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
                .frame(width: 12, height: 12)
                .padding(4)
                .contentShape(Rectangle())
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
