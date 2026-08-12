import SwiftUI

extension TreeRowRecord: Identifiable {}

/// Session branch tree overlay (SW10) — mirrors `ExtensionDialogView.swift`'s
/// `SelectDialogView` sheet skeleton (`NavigationStack` + `List` +
/// `.navigationTitle` + a cancellation-action Close button), not Slint's
/// custom scrim+card overlay — this branch's established native-sheet idiom.
/// Each row is indented by `depth`; forkable (`canFork`) rows show a "Fork"
/// button behind a confirmation `.alert` (native idiom, not Slint's 2.5s
/// double-tap chip — same precedent as the delete-confirmation fix).
struct TreeView: View {
    var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var rowPendingFork: TreeRowRecord?

    var body: some View {
        NavigationStack {
            List(model.treeRows) { row in
                HStack(spacing: 8) {
                    Circle()
                        .fill(row.isActive ? Color.accentColor : Color.clear)
                        .stroke(Color.secondary, lineWidth: 1)
                        .frame(width: 8, height: 8)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(row.summary)
                            .lineLimit(1)
                        if !row.label.isEmpty {
                            Text(row.label)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                    Spacer(minLength: 8)
                    if row.canFork {
                        Button("Fork") {
                            rowPendingFork = row
                        }
                        .buttonStyle(.borderless)
                    }
                }
                .padding(.leading, CGFloat(row.depth) * 16)
            }
            .navigationTitle("Session Tree")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") { dismiss() }
                }
            }
        }
        .frame(minWidth: 420, minHeight: 320)
        .task { await model.openTree() }
        .alert(
            "Fork Session?",
            isPresented: Binding(
                get: { rowPendingFork != nil },
                set: { presented in if !presented { rowPendingFork = nil } }
            ),
            presenting: rowPendingFork
        ) { row in
            Button("Cancel", role: .cancel) {}
            Button("Fork") {
                Task {
                    await model.forkFrom(entryId: row.id)
                    dismiss()
                }
            }
        } message: { row in
            Text(
                "This rewinds the session to before \"\(row.summary)\" and starts a new branch from there."
            )
        }
    }
}

#Preview {
    TreeView(model: AppModel())
}
