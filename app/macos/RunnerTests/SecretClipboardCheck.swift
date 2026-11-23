// Checks SecretClipboard.swift (factory/secret-clipboard) without the app,
// on a pasteboard of its own, never the person's clipboard:
//
//   cd app/macos && swiftc -parse-as-library -o /tmp/secret-clipboard-check \
//     Runner/SecretClipboard.swift RunnerTests/SecretClipboardCheck.swift \
//     && /tmp/secret-clipboard-check
import AppKit
import Darwin

@main
enum SecretClipboardCheck {
  static func main() {
    var failures = 0
    func check(_ ok: Bool, _ what: String, line: Int = #line) {
      if !ok {
        failures += 1
        print("FAILED (line \(line)): \(what)")
      }
    }

    let board = NSPasteboard(name: .init("claktory-secret-clipboard-check-\(getpid())"))
    defer { board.releaseGlobally() }
    let concealed = NSPasteboard.PasteboardType(SecretClipboard.concealedType)
    let transient = NSPasteboard.PasteboardType(SecretClipboard.transientType)

    // Copied: the text, marked for clipboard managers to skip.
    let token = SecretClipboard.copy("add-code-123", seconds: 1800, on: board)
    check(token == board.changeCount, "the token is the write's changeCount")
    check(board.string(forType: .string) == "add-code-123", "the text is there")
    let types = board.types ?? []
    check(types.contains(concealed), "marked concealed")
    check(types.contains(transient), "marked transient")

    // Cleared while it still holds that write: gone.
    SecretClipboard.clear(token: token, on: board)
    check(board.string(forType: .string) == nil, "cleared")

    // Something copied since is left alone.
    let again = SecretClipboard.copy("add-code-456", seconds: 1800, on: board)
    board.clearContents()
    board.setString("the person's own", forType: .string)
    SecretClipboard.clear(token: again, on: board)
    check(board.string(forType: .string) == "the person's own", "a later copy stays")

    // Swept (row 181): ours past its time goes; ours still working, and
    // anything else, stay. Only the types are read.
    let swept = SecretClipboard.copy("pipe-code-789", seconds: 600, on: board)
    check(
      (board.types ?? []).contains { $0.rawValue.hasPrefix(SecretClipboard.expiresPrefix) },
      "it carries when it stops working")
    check(!SecretClipboard.sweep(on: board), "not yet: left")
    if let ahead = SecretClipboard.expiresAhead(on: board) {
      let inSeconds = Double(ahead) / 1000 - Date().timeIntervalSince1970
      check(inSeconds > 590 && inSeconds <= 600, "its expiry ahead, to sweep again then")
    } else {
      check(false, "its expiry ahead, to sweep again then")
    }
    check(
      SecretClipboard.expiresAhead(now: Date().addingTimeInterval(601), on: board) == nil,
      "none ahead once past")
    check(board.string(forType: .string) == "pipe-code-789", "still there while it works")
    check(
      SecretClipboard.sweep(now: Date().addingTimeInterval(601), on: board),
      "past its time: cleared")
    check(board.string(forType: .string) == nil, "gone once dead")
    check(board.changeCount != swept, "cleared by the sweep")

    board.clearContents()
    board.setString("the person's own", forType: .string)
    check(
      !SecretClipboard.sweep(now: Date().addingTimeInterval(86400), on: board),
      "the person's own: never swept")
    check(board.string(forType: .string) == "the person's own", "the person's own stays")
    check(SecretClipboard.expiresAhead(on: board) == nil, "nothing of ours ahead")

    if failures == 0 { print("SecretClipboardCheck: all passed") }
    exit(failures == 0 ? 0 : 1)
  }
}
