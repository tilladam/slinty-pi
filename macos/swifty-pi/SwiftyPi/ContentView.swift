import SwiftUI

/// Top-level shell: a sidebar (project switcher + session list — browse,
/// switch project, new/delete/rename, and, as of SW3, click-to-resume) and
/// the chat detail pane.
struct ContentView: View {
    let model: AppModel
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        NavigationSplitView {
            SidebarView(model: model)
        } detail: {
            ChatView(model: model)
        }
        .task {
            model.start(dark: colorScheme == .dark)
        }
        .onChange(of: colorScheme) {
            model.setDarkMode(colorScheme == .dark)
        }
    }
}

#Preview {
    ContentView(model: AppModel())
}
