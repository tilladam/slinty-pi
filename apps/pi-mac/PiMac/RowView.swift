import AppKit
import SwiftUI

/// Renders one `RowRecord` — the SW3 rich-content counterpart to
/// slinty-pi's `ui/app.slint` per-row-kind components (`ProseRow`,
/// `CodeRow`, `TableBlock`, ...). `prose`/`quote` markdown goes through
/// SwiftUI's native `AttributedString(markdown:)`: `pi-render`'s segmenter
/// deliberately leaves inline markdown (bold/italic/links/lists) unresolved
/// for the UI toolkit's own renderer, and that's the one piece this native
/// init handles for us — see the SW3 plan's rendering-strategy rationale.
///
/// Copy-to-clipboard mirrors `app.slint`'s `CopyButton` exactly (see the
/// "Native NSPasteboard copy" plan section): always shown on `code` rows
/// (copies `row.text`, the fence-free code), shown once per message group
/// on `prose`/`quote`/`heading` when `row.first && !row.raw.isEmpty`
/// (copies `row.raw`, the full raw markdown source) — every other row kind
/// gets no copy affordance, matching Slint's own scope.
struct RowView: View {
    let row: RowRecord

    var body: some View {
        switch row.kind {
        case "prose", "quote":
            withGroupCopy(markdownText)
        case "heading":
            withGroupCopy(
                Text(row.text)
                    .font(headingFont)
                    .fontWeight(.bold)
                    .textSelection(.enabled)
            )
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
            userBubble
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

    /// Right-aligned, tinted bubble for the user's own prompts — mirrors
    /// `app.slint`'s `UserRow` (a leading stretch-spacer pushing the bubble
    /// right, `Palette.accent-background.transparentize(0.88)` background).
    /// `.accentColor.opacity(0.12)` is the native SwiftUI equivalent of that
    /// same light accent tint.
    private var userBubble: some View {
        HStack {
            Spacer(minLength: 40)
            Text(row.text)
                .fontWeight(.medium)
                .textSelection(.enabled)
                .padding(.horizontal, 11)
                .padding(.vertical, 8)
                .background(Color.accentColor.opacity(0.12), in: RoundedRectangle(cornerRadius: 8))
        }
    }

    /// Adds a trailing copy chip for `row.raw` when this row is the lead
    /// row of its message group and there's something to copy — mirrors
    /// `app.slint`'s `if entry.first && entry.raw != ""` gate on `ProseRow`/
    /// `HeadingRow`/`QuoteRow` exactly.
    @ViewBuilder
    private func withGroupCopy(_ content: some View) -> some View {
        if row.first, !row.raw.isEmpty {
            HStack(alignment: .top, spacing: 8) {
                content
                Spacer(minLength: 8)
                CopyButton(payload: row.raw)
            }
        } else {
            content
        }
    }
}

/// Click-to-copy chip mirroring `app.slint`'s `CopyButton`: flips to "✓"
/// for 1.4s after copying, matching the `.slint` component's own `Timer`.
private struct CopyButton: View {
    let payload: String
    @State private var copied = false

    var body: some View {
        Button {
            let pasteboard = NSPasteboard.general
            pasteboard.clearContents()
            pasteboard.setString(payload, forType: .string)
            copied = true
            Task {
                try? await Task.sleep(for: .milliseconds(1400))
                copied = false
            }
        } label: {
            Text(copied ? "✓" : "copy")
                .font(.caption2)
                .foregroundStyle(.secondary)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(.quaternary, in: RoundedRectangle(cornerRadius: 4))
        }
        .buttonStyle(.plain)
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
            HStack {
                if !row.lang.isEmpty {
                    Text(row.lang)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                CopyButton(payload: row.text)
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
