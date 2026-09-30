import Observation
import UIKit

/// Whether the software keyboard is on screen, so the player chrome inset at the
/// bottom can step aside instead of riding on top of the keyboard.
@MainActor
@Observable
final class KeyboardObserver {
  private(set) var isVisible = false

  @ObservationIgnored private var tokens: [NSObjectProtocol] = []

  init() {
    tokens = Self.observe { [weak self] visible in
      guard let self, self.isVisible != visible else { return }
      self.isVisible = visible
    }
  }

  private nonisolated static func observe(
    _ update: @escaping @MainActor @Sendable (Bool) -> Void
  ) -> [NSObjectProtocol] {
    let center = NotificationCenter.default
    return [
      center.addObserver(forName: UIResponder.keyboardWillShowNotification, object: nil, queue: .main) {
        notification in
        // With a hardware keyboard only the short shortcuts bar appears.
        let frame = (notification.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? NSValue)?.cgRectValue
        let visible = (frame?.height ?? 0) > 120
        MainActor.assumeIsolated { update(visible) }
      },
      center.addObserver(forName: UIResponder.keyboardWillHideNotification, object: nil, queue: .main) { _ in
        MainActor.assumeIsolated { update(false) }
      },
    ]
  }
}
