import SwiftUI

/// Top-level Settings-scene container: tabs between local-model management
/// and general app preferences.
struct SettingsView: View {
    @Bindable var model: AppModel

    var body: some View {
        TabView {
            GeneralPanelView(model: model)
                .tabItem { Label("General", systemImage: "gearshape") }
            ModelsPanelView(model: model)
                .tabItem { Label("Models", systemImage: "cpu") }
        }
        .frame(minWidth: 480, minHeight: 560)
    }
}

#Preview {
    SettingsView(model: AppModel())
}
