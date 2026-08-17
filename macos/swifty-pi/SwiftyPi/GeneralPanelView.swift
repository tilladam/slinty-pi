import SwiftUI

/// General app preferences: notifications and appearance — the SwiftUI
/// counterpart tab to `ModelsPanelView`, both hosted in `SettingsView`.
struct GeneralPanelView: View {
    /// `@Bindable`, not a plain `var` — same `Settings`-scene lifecycle
    /// hazard documented on `ModelsPanelView.model`.
    @Bindable var model: AppModel

    var body: some View {
        Form {
            Section("Appearance") {
                Picker("Appearance", selection: Binding(
                    get: { model.appearance },
                    set: { model.setAppearance($0) }
                )) {
                    ForEach(AppearanceMode.allCases) { mode in
                        Text(mode.label).tag(mode)
                    }
                }
                .pickerStyle(.segmented)
            }
            Section("Notifications") {
                Toggle("Enable notifications", isOn: Binding(
                    get: { model.notificationsEnabled },
                    set: { model.setNotificationsEnabled($0) }
                ))
            }
        }
        .formStyle(.grouped)
    }
}

#Preview {
    GeneralPanelView(model: AppModel())
}
