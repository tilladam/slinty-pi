import AppKit
import AVFoundation
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
    /// Density-driven force-expand for `"tool"` rows (Verbose mode) — inert
    /// for every other kind, so passing it uniformly at every call site is
    /// harmless. See the density control plan section.
    var forceToolExpanded: Bool = false

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
            ToolRowView(row: row, forceExpanded: forceToolExpanded)
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

    /// Adds a trailing copy chip and speak button for `row.raw` when this
    /// row is the lead row of its message group and there's something to
    /// copy — mirrors `app.slint`'s `if entry.first && entry.raw != ""`
    /// gate on `ProseRow`/`HeadingRow`/`QuoteRow` exactly. The speak button
    /// is disabled for messages that embed code/tables — read the raw
    /// markdown as-is, since a message this size is cheap to scan and it
    /// avoids threading sibling-row kinds down from `ChatView`.
    @ViewBuilder
    private func withGroupCopy(_ content: some View) -> some View {
        if row.first, !row.raw.isEmpty {
            HStack(alignment: .top, spacing: 8) {
                content
                Spacer(minLength: 8)
                CopyButton(payload: row.raw)
                SpeakButton(payload: row.raw, disabled: containsComplexContent(row.raw))
            }
        } else {
            content
        }
    }
}

/// Shared icon footprint for `CopyButton`/`SpeakButton` — SF Symbols have
/// different intrinsic glyph widths (e.g. `speaker.wave.2` is wider than
/// `checkmark`), so matching padding alone doesn't guarantee matching
/// button size; pinning both icons to the same frame does.
private let rowButtonIconSide: CGFloat = 14

/// Click-to-copy chip mirroring `app.slint`'s `CopyButton`: flips to a
/// checkmark for 1.4s after copying, matching the `.slint` component's own
/// `Timer`.
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
            Image(systemName: copied ? "checkmark" : "doc.on.doc")
                .font(.caption2)
                .frame(width: rowButtonIconSide, height: rowButtonIconSide)
                .foregroundStyle(.secondary)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(.quaternary, in: RoundedRectangle(cornerRadius: 4))
        }
        .buttonStyle(.plain)
        .help(copied ? "Copied" : "Copy")
    }
}

/// Shared, app-wide controller for the read-aloud feature — only one
/// utterance can ever play at once, matching how the OS's own "Start
/// Speaking" behaves. `speakingID` is the raw payload text itself (the same
/// string `CopyButton` copies), reused as an identity key so no separate id
/// plumbing is needed; two messages with byte-identical raw content
/// behaving as "the same" is a harmless, vanishingly rare edge case.
final class SpeechController: NSObject, ObservableObject {
    static let shared = SpeechController()

    @Published private(set) var speakingID: String?

    private let synthesizer = AVSpeechSynthesizer()

    private override init() {
        super.init()
        synthesizer.delegate = self
    }

    func toggle(id: String, text: String) {
        if speakingID == id {
            stop()
            return
        }
        synthesizer.stopSpeaking(at: .immediate)
        synthesizer.speak(AVSpeechUtterance(string: text))
        speakingID = id
    }

    func stop() {
        synthesizer.stopSpeaking(at: .immediate)
        speakingID = nil
    }
}

extension SpeechController: AVSpeechSynthesizerDelegate {
    func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didFinish utterance: AVSpeechUtterance) {
        speakingID = nil
    }

    func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didCancel utterance: AVSpeechUtterance) {
        speakingID = nil
    }
}

/// Click-to-speak chip beside `CopyButton` — reads the message aloud via
/// the built-in `AVSpeechSynthesizer`, stripping markdown syntax first
/// (the same `AttributedString(markdown:)` parse used for on-screen
/// rendering) so `**`/`[]()` aren't read literally. Visibly animates while
/// speaking via `.symbolEffect(.variableColor.iterative)` — a native,
/// built-in pulse, no custom animation code.
private struct SpeakButton: View {
    let payload: String
    let disabled: Bool
    @ObservedObject private var controller = SpeechController.shared

    private var isSpeaking: Bool { controller.speakingID == payload }

    var body: some View {
        Button {
            controller.toggle(id: payload, text: spokenText)
        } label: {
            Image(systemName: isSpeaking ? "speaker.wave.2.fill" : "speaker.wave.2")
                .symbolEffect(.variableColor.iterative, isActive: isSpeaking)
                .font(.caption2)
                .frame(width: rowButtonIconSide, height: rowButtonIconSide)
                .foregroundStyle(iconColor)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(.quaternary, in: RoundedRectangle(cornerRadius: 4))
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .help(disabled ? "Can't read code or tables aloud" : (isSpeaking ? "Stop speaking" : "Speak"))
    }

    private var spokenText: String {
        (try? AttributedString(markdown: payload)).map { String($0.characters) } ?? payload
    }

    private var iconColor: Color {
        if disabled { return .secondary.opacity(0.4) }
        return isSpeaking ? .accentColor : .secondary
    }
}

/// Cheap string-level check on the message's raw markdown — a fenced code
/// block or a GFM table separator row both mean "don't read this aloud."
/// Runs on `row.raw` directly rather than scanning sibling row kinds from
/// `ChatView`, since the raw text already contains everything needed.
private func containsComplexContent(_ raw: String) -> Bool {
    if raw.contains("```") { return true }
    return raw.range(of: #"(?m)^\s*\|?(\s*:?-{2,}:?\s*\|)+\s*:?-{2,}:?\s*\|?\s*$"#, options: [.regularExpression]) != nil
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
    let forceExpanded: Bool
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
            if expanded || forceExpanded, !row.detail.isEmpty {
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
