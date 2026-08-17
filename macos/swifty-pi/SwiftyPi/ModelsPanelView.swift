import SwiftUI

/// Local-model browse/manage panel: rapid-mlx status + cached models
/// (serve), a llama.cpp router's models (load/unload), Ollama detection
/// (bulk-add), and cloud API keys — the SwiftUI counterpart to
/// slinty-pi's `ui/models.slint` `ModelsOverlay`. Read/manage-only: no
/// composer "current model" picker and no server-dot health indicator —
/// SW4's explicit scope cut, see the project's swiftui-branch plan.
struct ModelsPanelView: View {
    /// `@Bindable`, not a plain `var` — hosted in a `Settings` scene rather
    /// than a sheet, where a plain stored property was observed to stop
    /// picking up `AppModel` changes after the window had been shown once.
    @Bindable var model: AppModel

    @State private var showHfSearch = false
    @State private var providerInput = ""
    @State private var keyInput = ""

    var body: some View {
        Form {
            rapidMlxSection
            routerSection
            ollamaSection
            authSection
        }
        .formStyle(.grouped)
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
                serverRow(panel.running)
                if let error = model.rapidMlxError {
                    Text(error)
                        .font(.caption)
                        .foregroundStyle(.red)
                }
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
                            actionButton(for: cached, running: panel.running)
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

    /// The running-server line. A server pi doesn't know about gets a
    /// warning: it's up, but pi can't route to it — usually a server started
    /// by hand with an alias that was never registered.
    @ViewBuilder
    private func serverRow(_ running: RunningServerRecord?) -> some View {
        HStack {
            Text("Server")
            Spacer()
            if let running {
                if !running.knownToPi {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(.orange)
                        .help("pi has no entry for this model — register it to use it")
                }
                Text(running.summary).foregroundStyle(.secondary)
            } else {
                Text("No server running").foregroundStyle(.secondary)
            }
        }
    }

    /// One action per state — the whole point of the panel's redesign:
    /// served → Stop, idle → Serve, unregistered → Register.
    @ViewBuilder
    private func actionButton(
        for cached: CachedModelRecord,
        running: RunningServerRecord?
    ) -> some View {
        switch cached.state {
        case .knownServed:
            let managed = running?.managed ?? false
            Button("Stop") {
                Task { await model.stopRapidMlx() }
            }
            .disabled(!managed)
            .help(
                managed
                    ? "Stop this rapid-mlx server"
                    : "This server wasn't started by SwiftyPi, so it can't be stopped from here"
            )
        case .knownIdle:
            Button("Serve") {
                Task { await model.serveRapidMlx(alias: cached.alias) }
            }
            .help("Start rapid-mlx serving this model")
        case .unknown:
            Button("Register") {
                Task { await model.registerRapidMlx(alias: cached.alias) }
            }
            .help("Add this model to pi's config so it can be selected — restarts pi")
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
