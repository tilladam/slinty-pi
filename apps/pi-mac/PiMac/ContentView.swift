import SwiftUI

/// Top-level shell: a sidebar (project switcher + session list — browse,
/// switch project, new/delete/rename; *not* resuming an existing session's
/// history, deferred until a rich-content-over-FFI answer exists, see
/// docs/plans SW3) and the chat detail pane.
struct ContentView: View {
    @State private var model = AppModel()

    var body: some View {
        NavigationSplitView {
            SidebarView(model: model)
        } detail: {
            ChatView(model: model)
        }
        .task {
            model.start()
        }
    }
}

#Preview {
    ContentView()
}
