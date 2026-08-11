import SwiftUI

/// M0-equivalent spike surface (mirrors `PRODUCT_PLAN.md`'s own M0: prove
/// the architecture end-to-end before real feature work): a composer, a
/// plain-text-appending transcript, and a status line. Deliberately no
/// markdown rendering, sessions, or model panel yet — see
/// docs/plans/SW1-ffi-spike-and-chat-window.md.
struct ChatView: View {
    @State private var viewModel = ChatViewModel()
    @State private var draft: String = ""

    var body: some View {
        VStack(spacing: 0) {
            ScrollViewReader { proxy in
                ScrollView {
                    Text(viewModel.transcript.isEmpty ? " " : viewModel.transcript)
                        .font(.system(.body, design: .monospaced))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding()
                        .id("transcript-end")
                }
                .onChange(of: viewModel.transcript) {
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

                if viewModel.isStreaming {
                    Button("Abort", role: .destructive) {
                        viewModel.abort()
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
        .task {
            viewModel.start()
        }
    }

    private var statusBar: some View {
        HStack {
            Circle()
                .fill(viewModel.isStreaming ? .green : .secondary)
                .frame(width: 8, height: 8)
            Text(viewModel.isStreaming ? "streaming…" : "idle")
                .font(.caption)
                .foregroundStyle(.secondary)
            Spacer()
            if let statusMessage = viewModel.statusMessage {
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
        viewModel.send(prompt)
        draft = ""
    }
}

#Preview {
    ChatView()
}
