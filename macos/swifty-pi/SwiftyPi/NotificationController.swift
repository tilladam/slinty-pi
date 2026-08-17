import AppKit
import UserNotifications
import os

/// Posts a native macOS notification when there's something the user would
/// want to know about while SwiftyPi isn't the active app — a turn finishing
/// (or erroring) and a pending extension-UI dialog (e.g. a permission
/// prompt) are the two triggers `AppModel` calls this for. Singleton, same
/// shape as `RowView.swift`'s `SpeechController`: one shared, app-wide
/// controller rather than something owned per-view.
@MainActor
final class NotificationController: NSObject, ObservableObject {
    static let shared = NotificationController()

    private let center = UNUserNotificationCenter.current()
    private let logger = Logger(subsystem: "dev.slinty-pi.swifty-pi", category: "notifications")

    /// Same key `AppModel` persists its General-tab toggle under — read
    /// directly rather than referencing `AppModel`, keeping this singleton
    /// self-contained.
    private static let notificationsEnabledDefaultsKey = "dev.slinty-pi.swifty-pi.notificationsEnabled"

    private override init() {
        super.init()
        center.delegate = self
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(handleDidBecomeActive),
            name: NSApplication.didBecomeActiveNotification,
            object: nil
        )
    }

    /// Called once from `AppModel`'s startup path — shows the system
    /// permission prompt the first time it's ever called. Failure is
    /// logged, not surfaced to the user, matching this app's existing
    /// "log plumbing failures, don't interrupt with them" posture.
    func requestAuthorizationIfNeeded() {
        Task {
            do {
                try await center.requestAuthorization(options: [.alert, .sound])
            } catch {
                logger.error("notification authorization failed: \(error.localizedDescription)")
            }
        }
    }

    func notifyTurnCompleted(project: String, hadError: Bool) {
        notify(
            id: "turn-complete",
            title: hadError ? "pi ran into an error" : "pi finished responding",
            body: project
        )
    }

    func notifyDialogPending(_ dialog: ExtensionDialogRecord) {
        let body = dialog.title ?? dialog.message ?? "Waiting for your response."
        notify(id: "dialog-\(dialog.id)", title: "pi needs your input", body: body)
    }

    /// Only ever posts while SwiftyPi isn't the frontmost app — if the user
    /// is already looking at it, a system notification would just be noise
    /// on top of what's already on screen. `id` is reused so a second
    /// turn-completion notification replaces rather than stacks; dialog
    /// notifications use a per-dialog id since more than one can queue.
    private func notify(id: String, title: String, body: String) {
        guard !NSApplication.shared.isActive else { return }
        guard UserDefaults.standard.object(forKey: Self.notificationsEnabledDefaultsKey) as? Bool ?? true else { return }
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.sound = .default
        let request = UNNotificationRequest(identifier: id, content: content, trigger: nil)
        center.add(request) { [logger] error in
            if let error {
                logger.error("failed to post notification: \(error.localizedDescription)")
            }
        }
    }

    @objc private func handleDidBecomeActive() {
        center.removeAllDeliveredNotifications()
    }
}

extension NotificationController: @preconcurrency UNUserNotificationCenterDelegate {
    /// Clicking a notification brings SwiftyPi to the front — the actual
    /// point of posting one in the first place.
    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        NSApp.activate(ignoringOtherApps: true)
        completionHandler()
    }

    /// Always show the banner, even in the rare case the app regained focus
    /// in the instant between `notify` checking `isActive` and the system
    /// presenting it — better a harmless extra banner than a silently
    /// dropped one.
    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .sound])
    }
}
