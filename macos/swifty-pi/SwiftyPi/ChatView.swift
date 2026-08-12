import AppKit
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
                            if AppModel.rowVisible(density: model.density, kind: row.kind) {
                                RowView(row: row, forceToolExpanded: model.density == 0)
                            }
                        }
                        ForEach(model.liveThinkingRows) { block in
                            if AppModel.rowVisible(density: model.density, kind: block.row.kind) {
                                RowView(row: block.row, forceToolExpanded: model.density == 0)
                            }
                        }
                        ForEach(model.liveToolRows) { block in
                            if AppModel.rowVisible(density: model.density, kind: block.row.kind) {
                                RowView(row: block.row, forceToolExpanded: model.density == 0)
                            }
                        }
                        ForEach(Array(model.streamingRows.enumerated()), id: \.offset) { _, row in
                            if AppModel.rowVisible(density: model.density, kind: row.kind) {
                                RowView(row: row, forceToolExpanded: model.density == 0)
                            }
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

            if !model.pendingAttachments.isEmpty {
                attachmentChips
            }

            HStack(alignment: .bottom, spacing: 8) {
                Button {
                    presentAttachPicker()
                } label: {
                    Image(systemName: "paperclip")
                }
                .help("Attach a file or image")

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
            .dropDestination(for: URL.self) { urls, _ in
                for url in urls {
                    model.attachPath(url.path)
                }
                return true
            }

            statusBar
        }
        .onChange(of: model.pendingComposerAppend) { _, newValue in
            guard newValue != nil, let text = model.consumePendingComposerAppend() else { return }
            draft += draft.isEmpty ? "@\(text)" : " @\(text)"
        }
        .frame(minWidth: 480, minHeight: 360)
        .navigationTitle(model.projectDisplayName)
        .navigationSubtitle(model.currentProject)
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
            // Same glass-background opt-out as the status dots above — a
            // flat dropdown reads as a lightweight display control here,
            // not a prominent action button.
            if #available(macOS 26.0, *) {
                ToolbarItem(placement: .primaryAction) {
                    densityPicker
                }
                .sharedBackgroundVisibility(.hidden)
            } else {
                ToolbarItem(placement: .primaryAction) {
                    densityPicker
                }
            }
        }
        .extensionDialogs(model: model)
    }

    private var statusDots: some View {
        HStack(spacing: 6) {
            Group {
                if model.isStreaming {
                    StreamingSpinner()
                } else {
                    Circle().stroke(Color.gray, lineWidth: 2)
                }
            }
            .frame(width: 12, height: 12)
            .padding(4)
            .contentShape(Rectangle())
            .help(model.isStreaming ? "Streaming" : "Idle")
            serverDotView
        }
    }

    /// Direct mode selection, replacing an earlier cycle-through-3-states
    /// button — a `Menu`-style `Picker` reads its current value from
    /// `model.density` and writes back through `AppModel.setDensity`.
    private var densityPicker: some View {
        Picker("Density", selection: densityBinding) {
            Text("Verbose").tag(0)
            Text("Normal").tag(1)
            Text("Summary").tag(2)
        }
        .pickerStyle(.menu)
        .labelsHidden()
        .help("Transcript density")
    }

    private var densityBinding: Binding<Int> {
        Binding(
            get: { model.density },
            set: { model.setDensity($0) }
        )
    }

    private var statusBar: some View {
        HStack(spacing: 6) {
            if let stats = model.sessionStats {
                usageRing(percent: stats.contextPercent)
                Text("\(stats.tokensLabel) tok · $\(String(format: "%.4f", stats.cost))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
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

    /// Session context-usage ring (SW8) — a clockwise-from-12-o'clock
    /// partial circle mirroring `app.slint`'s donut, turning red past 85%
    /// full. `percent` is 0-100 (confirmed against a real
    /// `get_session_stats()` response via `spike_check --thinking`, e.g.
    /// `context_percent=1.46` early in a session — not a 0-1 fraction).
    private func usageRing(percent: Float) -> some View {
        let fraction = CGFloat(min(max(percent / 100, 0), 1))
        return ZStack {
            Circle()
                .stroke(Color.secondary.opacity(0.25), lineWidth: 2)
            Circle()
                .trim(from: 0, to: fraction)
                .stroke(
                    percent > 85 ? Color.red : Color.accentColor,
                    style: StrokeStyle(lineWidth: 2, lineCap: .round)
                )
                .rotationEffect(.degrees(-90))
        }
        .frame(width: 12, height: 12)
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

    /// Queued-image chip row (SW9) — name + "×" per chip, mirroring
    /// `app.slint`'s pending-attachments strip.
    private var attachmentChips: some View {
        HStack(spacing: 6) {
            ForEach(Array(model.pendingAttachments.enumerated()), id: \.offset) { index, name in
                HStack(spacing: 4) {
                    Text(name)
                        .font(.caption)
                        .lineLimit(1)
                    Button {
                        model.removeAttachment(at: index)
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                            .foregroundStyle(.secondary)
                    }
                    .buttonStyle(.plain)
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(.quaternary, in: RoundedRectangle(cornerRadius: 6))
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
    }

    /// Native multi-file picker for the paperclip button — images queue as
    /// attachments, anything else appends an `@path` reference (see
    /// `AppModel.attachPath`'s doc comment).
    private func presentAttachPicker() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = false
        panel.canChooseFiles = true
        panel.allowsMultipleSelection = true
        panel.prompt = "Attach"
        guard panel.runModal() == .OK else { return }
        for url in panel.urls {
            model.attachPath(url.path)
        }
    }
}

/// Continuously-rotating partial-circle arc for the streaming indicator —
/// a custom `Circle().trim()` stroke rather than a plain `ProgressView()`,
/// since macOS's default circular spinner doesn't reliably pick up `.tint`.
private struct StreamingSpinner: View {
    @State private var rotation = 0.0

    var body: some View {
        Circle()
            .trim(from: 0, to: 0.7)
            .stroke(Color.green, style: StrokeStyle(lineWidth: 2, lineCap: .round))
            .rotationEffect(.degrees(rotation))
            .onAppear {
                withAnimation(.linear(duration: 0.8).repeatForever(autoreverses: false)) {
                    rotation = 360
                }
            }
    }
}

#Preview {
    ChatView(model: AppModel())
}
