import SwiftUI

@main
struct SwiftyPiApp: App {
    @State private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            ContentView(model: model)
                .preferredColorScheme(colorScheme(for: model.appearance))
        }
        .commands {
            CommandGroup(after: .newItem) {
                Menu("Open Recent Project") {
                    let recents = model.projects.filter { $0.displayPath != model.currentProject }
                    if recents.isEmpty {
                        Text("No Previous Projects")
                    } else {
                        ForEach(recents, id: \.displayPath) { project in
                            Button(project.displayPath) {
                                Task { await model.switchProject(to: project.displayPath) }
                            }
                        }
                    }
                }
                Button("Open Project…") {
                    Task { await model.promptSwitchProject() }
                }
                .keyboardShortcut("o")
            }
        }

        // A `Settings` scene gives the panel the platform-standard entry
        // points for free: "SwiftyPi > Settings…" in the app menu, bound to
        // ⌘,. `SidebarView`'s `SettingsLink` opens this same window.
        Settings {
            SettingsView(model: model)
        }
    }

    /// `nil` (system) leaves `\.colorScheme` alone; `.light`/`.dark` force
    /// it — `ContentView`'s `@Environment(\.colorScheme)` read picks up the
    /// override since it's applied above `ContentView` in the view tree.
    private func colorScheme(for mode: AppearanceMode) -> ColorScheme? {
        switch mode {
        case .light: .light
        case .dark: .dark
        case .system: nil
        }
    }
}
