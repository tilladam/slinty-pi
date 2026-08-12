import SwiftUI

extension ExtensionDialogRecord: Identifiable {}

/// Renders pi's extension-UI dialog protocol (`select`/`confirm`/`input`/
/// `editor`) — includes pi's sanctioned permission-gating pattern (a
/// `tool_call` extension + `confirm`/`select` dialog), which is not a
/// separate protocol, just this one rendered well. `.alert` for the two
/// lightweight kinds, `.sheet` for the two that need more room — mirrors
/// `SidebarView`'s existing rename-alert vs. models-sheet split.
///
/// Every dismissal path (including Esc, via a `role: .cancel` button, and
/// sheet swipe-dismiss, disabled below) always goes through an explicit
/// button that itself calls `AppModel.replyToCurrentDialog` — so there's no
/// separate "implicit dismiss" signal to reconcile against, avoiding a
/// double-reply race between a button action and `.alert`'s own
/// auto-dismiss.
struct ExtensionDialogModifier: ViewModifier {
    var model: AppModel
    @State private var inputText: String = ""
    @State private var editorText: String = ""

    private var alertDialog: ExtensionDialogRecord? {
        guard let dialog = model.currentDialog,
            dialog.method == "confirm" || dialog.method == "input"
        else { return nil }
        return dialog
    }

    private var sheetDialog: ExtensionDialogRecord? {
        guard let dialog = model.currentDialog,
            dialog.method == "select" || dialog.method == "editor"
        else { return nil }
        return dialog
    }

    func body(content: Content) -> some View {
        content
            .alert(
                alertDialog?.title ?? "pi",
                isPresented: Binding(get: { alertDialog != nil }, set: { _ in }),
                presenting: alertDialog
            ) { dialog in
                alertActions(for: dialog)
            } message: { dialog in
                Text(dialog.message ?? "")
            }
            .sheet(item: Binding(get: { sheetDialog }, set: { _ in })) { dialog in
                if dialog.method == "select" {
                    SelectDialogView(dialog: dialog, model: model)
                } else {
                    EditorDialogView(dialog: dialog, model: model, text: $editorText)
                }
            }
            .onChange(of: model.currentDialog) { _, newDialog in
                guard let newDialog else { return }
                inputText = newDialog.prefill ?? ""
                editorText = newDialog.prefill ?? ""
            }
    }

    @ViewBuilder
    private func alertActions(for dialog: ExtensionDialogRecord) -> some View {
        if dialog.method == "input" {
            TextField(dialog.placeholder ?? "", text: $inputText)
            Button("Cancel", role: .cancel) {
                model.replyToCurrentDialog(.cancelled)
            }
            Button("Submit") {
                model.replyToCurrentDialog(.value(value: inputText))
            }
        } else {
            Button("Deny", role: .cancel) {
                model.replyToCurrentDialog(.confirmed(confirmed: false))
            }
            .keyboardShortcut("n", modifiers: [])
            Button("Allow") {
                model.replyToCurrentDialog(.confirmed(confirmed: true))
            }
            .keyboardShortcut("y", modifiers: [])
        }
    }
}

/// `select` — the real-world-observed shape (verified against this
/// project's own installed `pi-permission-system` extension during SW5
/// planning): a long, multi-line prompt plus a handful of options, not a
/// short label suited to `navigationTitle`. The prompt renders as a normal
/// paragraph above the option list instead.
private struct SelectDialogView: View {
    let dialog: ExtensionDialogRecord
    var model: AppModel

    private var options: [String] { dialog.options ?? [] }

    var body: some View {
        NavigationStack {
            List {
                if let prompt = dialog.title ?? dialog.message {
                    Text(prompt)
                        .font(.callout)
                }
                ForEach(Array(options.enumerated()), id: \.offset) { index, option in
                    optionRow(index: index, option: option)
                }
            }
            .navigationTitle("Choose an Option")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        model.replyToCurrentDialog(.cancelled)
                    }
                    .keyboardShortcut(.cancelAction)
                }
            }
        }
        .frame(minWidth: 380, minHeight: 260)
        .interactiveDismissDisabled()
        .background {
            // Wires the actual y/s/n key bindings off the same `mnemonic`
            // function the visible "(y)"/"(s)"/"(n)" hint above uses, so
            // the hint and the binding can never disagree. Hidden buttons
            // are the standard SwiftUI idiom for a keyboard-only affordance
            // with no visual footprint of its own.
            ForEach(["y", "s", "n"] as [Character], id: \.self) { letter in
                if let match = options.first(where: { mnemonic(for: $0) == letter }) {
                    Button("") {
                        model.replyToCurrentDialog(.value(value: match))
                    }
                    .keyboardShortcut(KeyEquivalent(letter), modifiers: [])
                    .hidden()
                }
            }
        }
    }

    /// Split out of `body`'s `ForEach` closure — inlined, the combination
    /// of the numeric badge/mnemonic-hint conditionals and the
    /// `.keyboardShortcut` call was too much for the type-checker to
    /// resolve in reasonable time. `.keyboardShortcut(_:modifiers:)` takes
    /// a non-optional `KeyEquivalent`, so the 10th-and-beyond row (no
    /// number shortcut) is a real `if`/`else` branch, not an optional
    /// passed through.
    @ViewBuilder
    private func optionRow(index: Int, option: String) -> some View {
        if index < 9 {
            Button {
                model.replyToCurrentDialog(.value(value: option))
            } label: {
                optionLabel(index: index, option: option)
            }
            .keyboardShortcut(KeyEquivalent(Character("\(index + 1)")), modifiers: [])
        } else {
            Button {
                model.replyToCurrentDialog(.value(value: option))
            } label: {
                optionLabel(index: index, option: option)
            }
        }
    }

    @ViewBuilder
    private func optionLabel(index: Int, option: String) -> some View {
        HStack {
            if index < 9 {
                Text("\(index + 1)")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
                    .frame(width: 16, alignment: .trailing)
            }
            Text(option)
            if let letter = mnemonic(for: option) {
                Text("(\(String(letter)))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
        }
    }

    /// Best-effort mnemonic for an option's text — never required (the
    /// numbered shortcuts above always work regardless of option wording),
    /// just a discoverable shortcut when an option's intent is obvious.
    /// Checked in this order so a hypothetical "Allow for session" option
    /// resolves to `s`, not `y`.
    private func mnemonic(for option: String) -> Character? {
        let lower = option.lowercased()
        if lower.contains("session") { return "s" }
        if lower.contains("allow") || lower.contains("yes") { return "y" }
        if lower.contains("deny") || lower.contains("no") { return "n" }
        return nil
    }
}

private struct EditorDialogView: View {
    let dialog: ExtensionDialogRecord
    var model: AppModel
    @Binding var text: String

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 8) {
                if let prompt = dialog.title ?? dialog.message {
                    Text(prompt)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
                TextEditor(text: $text)
                    .font(.system(.body, design: .monospaced))
            }
            .padding(8)
            .navigationTitle("Edit Text")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        model.replyToCurrentDialog(.cancelled)
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Submit") {
                        model.replyToCurrentDialog(.value(value: text))
                    }
                }
            }
        }
        .frame(minWidth: 480, minHeight: 320)
        .interactiveDismissDisabled()
    }
}

extension View {
    func extensionDialogs(model: AppModel) -> some View {
        modifier(ExtensionDialogModifier(model: model))
    }
}
