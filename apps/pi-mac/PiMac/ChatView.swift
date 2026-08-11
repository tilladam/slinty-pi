import SwiftUI

/// Chat detail pane: composer, plain-text-appending transcript, streaming
/// status. Deliberately no markdown rendering yet — see
/// docs/plans/SW1-ffi-spike-and-chat-window.md.
struct ChatView: View {
    var model: AppModel
    @State private var draft: String = ""

    var body: some View {
        VStack(spacing: 0) {
            ScrollViewReader { proxy in
                ScrollView {
                    Text(model.transcript.isEmpty ? " " : model.transcript)
                        .font(.system(.body, design: .monospaced))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding()
                        .id("transcript-end")
                }
                .onChange(of: model.transcript) {
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
    }

    private var statusBar: some View {
        HStack {
            Circle()
                .fill(model.isStreaming ? .green : .secondary)
                .frame(width: 8, height: 8)
            Text(model.isStreaming ? "streaming…" : "idle")
                .font(.caption)
                .foregroundStyle(.secondary)
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
