import SwiftUI

/// Local-model browse/manage panel: rapid-mlx status + cached models
/// (serve), a llama.cpp router's models (load/unload), Ollama detection
/// (bulk-add), and cloud API keys — the SwiftUI counterpart to
/// slinty-pi's `ui/models.slint` `ModelsOverlay`. Read/manage-only: no
/// composer "current model" picker and no server-dot health indicator —
/// SW4's explicit scope cut, see the project's swiftui-branch plan.
struct ModelsPanelView: View {
    var model: AppModel
    @Environment(\.dismiss) private var dismiss

    @State private var showHfSearch = false
    @State private var providerInput = ""
    @State private var keyInput = ""

    var body: some View {
        NavigationStack {
            Form {
                rapidMlxSection
                routerSection
                ollamaSection
                authSection
            }
            .formStyle(.grouped)
            .navigationTitle("Models")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") { dismiss() }
                }
            }
        }
        .frame(minWidth: 480, minHeight: 560)
        .task {
            await model.refreshModelsPanel()
        }
        .sheet(isPresented: $showHfSearch) {
            HfSearchView(model: model)
        }
    }

    @ViewBuilder
    private var rapidMlxSection: some View {
        Section("rapid-mlx") {
            if let panel = model.rapidMlxPanel {
                LabeledContent("Version", value: panel.version ?? "not detected — install with `brew install rapid-mlx`")
                LabeledContent("Server", value: panel.runningSummary ?? "No server running")
                if panel.cached.isEmpty {
                    Text("No cached models yet — run `rapid-mlx pull <alias>` to download one.")
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(Array(panel.cached.enumerated()), id: \.offset) { _, cached in
                        HStack {
                            VStack(alignment: .leading) {
                                Text(cached.alias).fontWeight(.semibold)
                                Text("\(cached.hfRepo) · \(cached.size)")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            Spacer()
                            Text(cached.fitLabel)
                                .font(.caption)
                                .padding(.horizontal, 6)
                                .background(.quaternary, in: Capsule())
                            Button("Serve") {
                                Task { await model.serveRapidMlx(alias: cached.alias) }
                            }
                        }
                    }
                }
                if panel.catalogCount > 0 {
                    Text("\(panel.catalogCount) aliases available in the rapid-mlx catalog — download more with `rapid-mlx pull <alias>`.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            } else {
                ProgressView()
            }
        }
    }

    @ViewBuilder
    private var routerSection: some View {
        Section("llama.cpp router") {
            if let panel = model.routerPanel {
                HStack {
                    Text(routerStatusText(panel))
                    Spacer()
                    Button("Download model…") { showHfSearch = true }
                }
                if panel.models.isEmpty {
                    Text("No router models detected.")
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(Array(panel.models.enumerated()), id: \.offset) { _, row in
                        HStack {
                            VStack(alignment: .leading) {
                                Text(row.id).fontWeight(.semibold)
                                Text(row.statusLabel)
                                    .font(.caption)
                                    .foregroundStyle(row.loaded ? Color.accentColor : Color.secondary)
                            }
                            Spacer()
                            if !row.busy {
                                Button(row.loaded ? "unload" : "load") {
                                    Task {
                                        if row.loaded {
                                            await model.unloadRouterModel(id: row.id)
                                        } else {
                                            await model.loadRouterModel(id: row.id)
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                ProgressView()
            }
        }
    }

    private func routerStatusText(_ panel: RouterPanelRecord) -> String {
        switch panel.statusLabel {
        case "ready": return "connected · \(panel.baseUrl)"
        case "loading": return "starting up · \(panel.baseUrl)"
        default: return "not reachable at \(panel.baseUrl)"
        }
    }

    @ViewBuilder
    private var ollamaSection: some View {
        Section("Ollama") {
            if let panel = model.ollamaPanel {
                HStack {
                    Text(panel.detected ? panel.summary : "not detected")
                        .foregroundStyle(.secondary)
                    Spacer()
                    if panel.modelCount > 0 {
                        Button("Add all to pi") {
                            Task { await model.addOllamaToPi() }
                        }
                    }
                }
            } else {
                ProgressView()
            }
        }
    }

    private var authSection: some View {
        Section {
            if model.authEntries.isEmpty {
                Text("No credentials stored yet.")
                    .foregroundStyle(.secondary)
            } else {
                ForEach(model.authEntries, id: \.self) { entry in
                    Text(entry)
                }
            }
            HStack {
                TextField("provider (e.g. anthropic)", text: $providerInput)
                SecureField("API key", text: $keyInput)
                Button("Save") {
                    let provider = providerInput
                    let key = keyInput
                    Task {
                        await model.saveApiKey(provider: provider, key: key)
                        keyInput = ""
                    }
                }
                .disabled(providerInput.isEmpty || keyInput.isEmpty)
            }
        } header: {
            Text("Cloud API keys")
        } footer: {
            Text("Stored in ~/.pi/agent/auth.json (0600). $ENV/!command and OAuth entries are read-only here — manage those in the file or via `pi /login`.")
                .font(.caption)
        }
    }
}

#Preview {
    ModelsPanelView(model: AppModel())
}
