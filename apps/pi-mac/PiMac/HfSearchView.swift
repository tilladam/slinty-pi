import SwiftUI
import AppKit

/// Hugging Face GGUF search sub-panel, opened from `ModelsPanelView`'s
/// router section — the SwiftUI counterpart to `ui/models.slint`'s
/// `HfSearchOverlay`. Enter-to-search (no live-as-you-type), matching the
/// Slint app.
struct HfSearchView: View {
    var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""

    var body: some View {
        NavigationStack {
            List(model.hfResults, id: \.id) { result in
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text(result.id).fontWeight(.semibold)
                        Spacer()
                        Text("\(result.downloads)")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    if result.gated {
                        Button {
                            if let url = URL(string: "https://huggingface.co/\(result.id)") {
                                NSWorkspace.shared.open(url)
                            }
                        } label: {
                            Text("⚠ gated — requires accepting a license on Hugging Face")
                                .font(.caption)
                                .foregroundStyle(.orange)
                        }
                        .buttonStyle(.plain)
                    }
                    ScrollView(.horizontal) {
                        HStack {
                            ForEach(result.quants, id: \.self) { quant in
                                Button(quant) {
                                    Task { await model.downloadHfModel("\(result.id):\(quant)") }
                                }
                                .buttonStyle(.bordered)
                                .controlSize(.small)
                            }
                        }
                    }
                }
                .padding(.vertical, 4)
            }
            .overlay {
                if model.hfResults.isEmpty {
                    ContentUnavailableView(
                        "Search above (press Enter) to find GGUF models on Hugging Face.",
                        systemImage: "magnifyingglass"
                    )
                }
            }
            .navigationTitle("Download a model")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") { dismiss() }
                }
            }
            .searchable(text: $query, prompt: "Search Hugging Face for a GGUF model…")
            .onSubmit(of: .search) {
                Task { await model.searchHfModels(query: query) }
            }
        }
        .frame(minWidth: 420, minHeight: 420)
    }
}

#Preview {
    HfSearchView(model: AppModel())
}
