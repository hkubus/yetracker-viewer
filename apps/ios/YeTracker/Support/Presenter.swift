import SafariServices
import UIKit

/// Presents UIKit controllers on top of whatever is showing, including sheets.
/// SwiftUI can only present one sheet per level, and a download can finish
/// while Now Playing is open, so these go through UIKit instead.
@MainActor
enum Presenter {
  /// In-app browser for catalog source links (http/https only).
  static func presentSafari(_ url: URL) {
    guard let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https" else { return }
    let controller = SFSafariViewController(url: url)
    controller.dismissButtonStyle = .close
    present(controller)
  }

  /// The share sheet for a downloaded file (Save to Files, AirDrop, …).
  static func presentShareSheet(for fileURL: URL, completion: @escaping () -> Void = {}) {
    let controller = UIActivityViewController(activityItems: [fileURL], applicationActivities: nil)
    controller.completionWithItemsHandler = { _, _, _, _ in completion() }
    present(controller)
  }

  private static func present(_ controller: UIViewController) {
    guard let top = topViewController() else { return }
    if let popover = controller.popoverPresentationController {
      // iPad: anchor near the bottom centre, where the player and banners live.
      popover.sourceView = top.view
      popover.sourceRect = CGRect(x: top.view.bounds.midX, y: top.view.bounds.maxY - 120, width: 1, height: 1)
      popover.permittedArrowDirections = []
    }
    top.present(controller, animated: true)
  }

  private static func topViewController() -> UIViewController? {
    let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
    let scene = scenes.first { $0.activationState == .foregroundActive } ?? scenes.first
    var top = scene?.keyWindow?.rootViewController
    while let presented = top?.presentedViewController, !presented.isBeingDismissed {
      top = presented
    }
    return top
  }
}
