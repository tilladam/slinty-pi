import SwiftUI
import AppKit

/// Session browsing + lifecycle actions: list, switch project, new, delete,
/// rename. Deliberately *not* click-to-resume — non-active rows are
/// informational only (dimmed, no tap action) until a later milestone
/// decides how to hydrate/render an existing session's history across the
/// FFI boundary (see docs/plans SW3).
struct SidebarView: View {
    var model: AppModel

    @State private var isRenaming = false
    @State private var renameText = ""

    var body: some View {
        List(model.sessions, id: \.path) { session in
            SessionRowView(session: session)
                .contextMenu {
                    if session.active {
                        Button("Rename…") {
                            renameText = session.title
                            isRenaming = true
                        }
                    }
                    Button("Delete", role: .destructive) {
                        Task { await model.deleteSession(session.path) }
                    }
                }
        }
        .listStyle(.sidebar)
        .navigationTitle(projectDisplayName)
        .toolbar {
            ToolbarItem {
                Button {
                    Task { await model.startNewSession() }
                } label: {
                    Label("New Session", systemImage: "square.and.pencil")
                }
                .help("Start a new session in \(projectDisplayName)")
            }
            ToolbarItem {
                Button {
                    pickProject()
                } label: {
                    Label("Switch Project", systemImage: "folder")
                }
                .help("Switch to a different project directory")
            }
        }
        .alert("Rename Session", isPresented: $isRenaming) {
            TextField("Name", text: $renameText)
            Button("Cancel", role: .cancel) {}
            Button("Rename") {
                Task { await model.renameActiveSession(to: renameText) }
            }
        }
    }

    private var projectDisplayName: String {
        (model.currentProject as NSString).lastPathComponent
    }

    private func pickProject() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = "Switch"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        Task { await model.switchProject(to: url.path) }
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
