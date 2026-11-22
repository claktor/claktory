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

    if failures == 0 { print("SecretClipboardCheck: all passed") }
    exit(failures == 0 ? 0 : 1)
  }
}
