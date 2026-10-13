import AppKit
import FastZipKit

struct ExternalRequestReview {
    let action: String
    let paths: [String]
    let destinations: [String]
    let deletesOriginals: Bool
}

@MainActor
enum ExternalRequestDialog {
    static func confirm(_ review: ExternalRequestReview) -> Bool {
        let alert = NSAlert()
        alert.messageText = L10n.t("Allow this external request?")
        alert.informativeText = L10n.t("Review every source and destination. Continue only if you requested this action.")
        alert.alertStyle = .warning
        var lines = [review.action, "", L10n.t("Source paths:")]
        // Escape control characters in filenames so one path cannot impersonate
        // another source, a destination or a line of the approval message.
        lines += review.paths.map(\.debugDescription)
        if !review.destinations.isEmpty {
            lines += ["", L10n.t("Destination paths:")] + review.destinations.map(\.debugDescription)
        }
        lines += ["", review.deletesOriginals
            ? L10n.t("The original files will be deleted after successful completion.")
            : L10n.t("The original files will be kept.")]
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 560, height: 240))
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        let text = NSTextView(frame: scroll.bounds)
        text.isEditable = false
        text.isSelectable = true
        text.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        text.string = lines.joined(separator: "\n")
        text.textContainer?.widthTracksTextView = true
        text.isVerticallyResizable = true
        scroll.documentView = text
        alert.accessoryView = scroll
        alert.addButton(withTitle: L10n.t("Cancel"))
        alert.addButton(withTitle: L10n.t("Continue"))
        NSApp.activate(ignoringOtherApps: true)
        return alert.runModal() == .alertSecondButtonReturn
    }
}
