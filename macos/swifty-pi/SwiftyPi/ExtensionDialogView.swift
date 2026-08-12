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
            Button("Allow") {
                model.replyToCurrentDialog(.confirmed(confirmed: true))
            }
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

    var body: some View {
        NavigationStack {
            List {
                if let prompt = dialog.title ?? dialog.message {
                    Text(prompt)
                        .font(.callout)
                }
                ForEach(dialog.options ?? [], id: \.self) { option in
                    Button(option) {
                        model.replyToCurrentDialog(.value(value: option))
                    }
                }
            }
            .navigationTitle("Choose an Option")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        model.replyToCurrentDialog(.cancelled)
                    }
                }
            }
        }
        .frame(minWidth: 380, minHeight: 260)
        .interactiveDismissDisabled()
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
