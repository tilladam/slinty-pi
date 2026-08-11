import SwiftUI

/// Renders one `RowRecord` — the SW3 rich-content counterpart to
/// slinty-pi's `ui/app.slint` per-row-kind components (`ProseRow`,
/// `CodeRow`, `TableBlock`, ...). `prose`/`quote` markdown goes through
/// SwiftUI's native `AttributedString(markdown:)`: `pi-render`'s segmenter
/// deliberately leaves inline markdown (bold/italic/links/lists) unresolved
/// for the UI toolkit's own renderer, and that's the one piece this native
/// init handles for us — see the SW3 plan's rendering-strategy rationale.
struct RowView: View {
    let row: RowRecord

    var body: some View {
        switch row.kind {
        case "prose", "quote":
            markdownText
        case "heading":
            Text(row.text)
                .font(headingFont)
                .fontWeight(.bold)
                .textSelection(.enabled)
        case "code":
            CodeBlockView(row: row)
        case "table":
            TableBlockView(rows: row.tableRows)
        case "rule":
            Divider()
        case "thinking":
            ThinkingRowView(row: row)
        case "tool":
            ToolRowView(row: row)
        case "user":
            Text(row.text)
                .fontWeight(.medium)
                .textSelection(.enabled)
        case "error":
            Text(row.text)
                .foregroundStyle(.red)
                .textSelection(.enabled)
        default: // "info" and any future/unrecognized kind
            Text(row.text)
                .font(.caption)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
        }
    }

    private var markdownText: some View {
        let source = row.markdown ?? row.text
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        let attributed = (try? AttributedString(markdown: source, options: options)) ?? AttributedString(source)
        return Text(attributed).textSelection(.enabled)
    }

    private var headingFont: Font {
        switch row.level {
        case 1: return .title
        case 2: return .title2
        case 3: return .title3
        default: return .headline
        }
    }
}

private struct ThinkingRowView: View {
    let row: RowRecord

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Label(row.running ? "Thinking…" : "Thought", systemImage: "brain")
                .font(.caption)
                .foregroundStyle(.secondary)
            if !row.text.isEmpty {
                Text(row.text)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
        }
    }
}

private struct ToolRowView: View {
    let row: RowRecord
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Button {
                expanded.toggle()
            } label: {
                HStack(spacing: 6) {
                    Text(row.text)
                        .font(.system(.caption, design: .monospaced))
                    if row.running {
                        ProgressView().controlSize(.small)
                    }
                    if !row.elapsed.isEmpty {
                        Text(row.elapsed)
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                }
            }
            .buttonStyle(.plain)
            if expanded, !row.detail.isEmpty {
                Text(row.detail)
                    .font(.system(.caption2, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
        }
    }
}

private struct CodeBlockView: View {
    let row: RowRecord

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if !row.lang.isEmpty {
                Text(row.lang)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(row.codeLines.enumerated()), id: \.offset) { _, line in
                    codeLineText(line)
                }
            }
            .padding(8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.quaternary, in: RoundedRectangle(cornerRadius: 6))
        }
    }

    /// Concatenated per-span `Text`, each carrying its own pre-resolved
    /// color (`highlight_lines` already resolved every span to a concrete
    /// RGB triple — no theme data needed here) — SwiftUI composes styled
    /// runs across `Text + Text`, same shape as Slint's `for span in
    /// line.spans` but expressed as concatenation instead of a layout.
    private func codeLineText(_ line: CodeLineRecord) -> Text {
        guard !line.spans.isEmpty else {
            return Text(" ").font(.system(.caption, design: .monospaced))
        }
        return line.spans
            .map { span in
                Text(span.text).foregroundColor(color(for: span))
            }
            .reduce(Text(""), +)
            .font(.system(.caption, design: .monospaced))
    }

    private func color(for span: ColoredSpanRecord) -> Color {
        Color(
            red: Double(span.red) / 255,
            green: Double(span.green) / 255,
            blue: Double(span.blue) / 255
        )
    }
}

private struct TableBlockView: View {
    let rows: [[TableCellRecord]]

    var body: some View {
        Grid(alignment: .leading) {
            ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                GridRow {
                    ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                        Text(cell.text)
                            .font(cell.header ? .caption.bold() : .caption)
                            .padding(4)
                    }
                }
            }
        }
    }
}

#Preview {
    RowView(row: RowRecord(
        kind: "code", markdown: nil, text: "fn main() {}", lang: "rust", level: 0,
        detail: "", running: false, elapsed: "", first: true, raw: "",
        codeLines: [CodeLineRecord(spans: [
            ColoredSpanRecord(text: "fn main() {}", red: 100, green: 150, blue: 200)
        ])],
        tableRows: []
    ))
    .padding()
}
