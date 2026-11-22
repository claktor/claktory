// The notification extension (#273): every push to this iPhone comes through
// Apple as "Claktory has something for you" with the Claktory's sealed text
// in "c". This opens it with the key kept for that Claktory
// (Runner/Push.swift) and shows what it says, under the Claktory's name. What
// doesn't open, is a replay or is too old keeps the generic line.
//
// It never uses the network and reads only the notification keys.

import UserNotifications

final class NotificationService: UNNotificationServiceExtension {
  private var handler: ((UNNotificationContent) -> Void)?
  private var generic: UNNotificationContent?

  override func didReceive(
    _ request: UNNotificationRequest,
    withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
  ) {
    handler = contentHandler
    generic = PushOpener.generic(request.content)
    contentHandler(PushOpener.content(for: request.content, keys: KeychainPushKeys(), now: Date()))
    handler = nil
  }

  /// Out of time: the generic line.
  override func serviceExtensionTimeWillExpire() {
    if let handler, let generic { handler(generic) }
    handler = nil
  }
}
