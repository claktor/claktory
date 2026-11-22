// The person's review of one browser action (B5b), the Mac app's side: an
// agent asked to click, fill or navigate in the Chrome tab it may use; the
// extension described that action and the helper checked the description
// against the very command (its hash); here the person allows it once,
// denies it, or stops the session.
//
// The decision comes only from the person's own click in the native review
// panel (BrowserReviewPanel.swift), never from Dart, the backend, another
// device or a timer: the only way to make one is a [ReviewClick], which only
// [ReviewDesk.press] makes, and only for a review on screen for at least
// [ReviewDesk.armAfter], not lapsed, once. Dart may bring the panel forward
// and hears what was decided; it can't decide.
//
// Foundation only, so it's checked without the app
// (RunnerTests/BrowserBrokerCheck.swift).
import Foundation

/// The person's decision on one review, as only [ReviewDesk.press] makes it;
/// [BrowserBroker.decide] sends nothing without one.
struct ReviewClick {
  let helper: Int
  let command: Int
  let hash: String
  let allow: Bool

  fileprivate init(helper: Int, command: Int, hash: String, allow: Bool) {
    self.helper = helper
    self.command = command
    self.hash = hash
    self.allow = allow
  }
}

/// The panel's fixed words, Trish's, from Dart at start (labels only: what
/// the action is comes from the verified review). A missing one shows its
/// key, so it's seen at once.
struct ReviewWords {
  var words: [String: String] = [:]

  /// The keys the panel uses; Dart passes each.
  static let keys = [
    "title_click", "title_fill", "title_navigate", "title_other",
    "page", "target", "shows", "text", "destination",
    "warning_may_send_as_typed", "warning_ends_session", "warning_may_ask_permission",
    "warning_submits_form", "unshown_details", "leaves_other_origin",
    "marks_lead", "mark_bidi_control", "mark_zero_width", "mark_mixed_script", "mark_line_break",
    "allow", "deny", "stop", "seconds_left", "expired", "go_back", "escape_denies",
  ]

  /// [key]'s words, each {name} in them replaced from [values].
  func say(_ key: String, _ values: [String: String] = [:]) -> String {
    var text = words[key] ?? key
    for (name, value) in values { text = text.replacingOccurrences(of: "{\(name)}", with: value) }
    return text
  }
}

/// What the panel shows: the verified review's action fields, and who asked
/// and where from the session, never from the page.
struct ReviewContent: Equatable {
  let command: Int
  let hash: String
  let expiresMs: Int64
  let op: String
  /// The agent's name (from the session's binding) and the session's origin
  /// (the one the helper pinned, its scheme kept), both one line.
  let agent: String
  let origin: String
  /// (label key, value) in order; page-supplied names quoted and cut.
  let rows: [Row]
  /// What a fill types, whole: the person sees exactly what will be typed.
  let text: String?
  /// What it may lead to, for the warning box, as word keys (warning_…).
  let warnings: [String]
  /// What the page's names hold that can make them read as something else,
  /// for the warning box under its lead line, as word keys (mark_…).
  let marks: [String]
  /// Where it leaves to, shown with its destination, as word keys
  /// (leaves_…).
  let leaves: [String]

  struct Row: Equatable {
    let key: String
    let value: String
  }

  /// The action's fields this version shows (browser-extension/PROTOCOL.md).
  static let known: Set<String> = [
    "op", "origin", "path", "role", "name", "shows", "marks", "text", "destination", "leaves",
    "submits_form", "warnings",
  ]

  /// Longest a page-supplied name shows, in characters.
  static let nameLimit = 80
  /// Longest a path shows.
  static let pathLimit = 160

  /// From a review frame as the helper relayed it (it checked its hash
  /// against the command, its size and that it's escaped); nil when it
  /// isn't one.
  static func parse(_ frame: [String: Any], agent: String, origin: String) -> ReviewContent? {
    guard frame["type"] as? String == "review",
      let command = frame["id"] as? Int,
      let hash = frame["hash"] as? String,
      let expires = (frame["expires_ms"] as? NSNumber)?.int64Value,
      let action = frame["action"] as? [String: Any],
      let op = action["op"] as? String
    else { return nil }
    var rows: [Row] = []
    func add(_ key: String, _ value: String?) {
      if let value, !value.isEmpty { rows.append(Row(key: key, value: value)) }
    }
    add("page", (action["path"] as? String).map { cut(oneLine($0), pathLimit) })
    let role = (action["role"] as? String).map(oneLine)
    let name = (action["name"] as? String).map { quoted($0) }
    add("target", [role, name].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " "))
    add("shows", (action["shows"] as? String).map { quoted($0) })
    add("destination", (action["destination"] as? String).map { cut(oneLine($0), pathLimit) })
    func codes(_ value: Any?) -> [String] {
      switch value {
      case let one as String: return [one]
      case let list as [Any]: return list.compactMap { $0 as? String }
      default: return []
      }
    }
    var warnings = codes(action["warnings"]).map { "warning_\(oneLine($0))" }
    // A field this version doesn't know (an extension newer than the app)
    // is said, never dropped silently: the panel isn't showing everything.
    if !Set(action.keys).isSubset(of: known) { warnings.append("unshown_details") }
    return ReviewContent(
      command: command, hash: hash, expiresMs: expires, op: op,
      agent: cut(oneLine(agent), nameLimit), origin: ascii(origin),
      rows: rows,
      text: (action["text"] as? String).map { typed($0) },
      warnings: warnings,
      marks: codes(action["marks"]).map { "mark_\(oneLine($0))" },
      leaves: codes(action["leaves"]).map { "leaves_\(oneLine($0))" })
  }

  /// A page-supplied name: one line, cut, in quotes, so it never reads as
  /// the app's own words.
  static func quoted(_ text: String) -> String {
    "“\(cut(oneLine(text), nameLimit))”"
  }

  static func cut(_ text: String, _ limit: Int) -> String {
    text.count <= limit ? text : String(text.prefix(limit - 1)) + "…"
  }

  /// Control, bidi and zero-width characters as ⟦U+XXXX⟧ (the extension
  /// escaped them already; this holds even if it didn't), on one line.
  static func oneLine(_ text: String) -> String {
    escapedControls(text).replacingOccurrences(of: "\n", with: "⏎")
  }

  /// [text] with every control, bidi and zero-width character but a line
  /// break shown as ⟦U+XXXX⟧.
  static func escapedControls(_ text: String) -> String {
    var out = ""
    for scalar in text.unicodeScalars {
      let v = scalar.value
      let hidden =
        (v < 0x20 && v != 0x0A) || (0x7F...0x9F).contains(v) || (0x200B...0x200F).contains(v)
        || (0x202A...0x202E).contains(v) || (0x2060...0x2069).contains(v) || v == 0xFEFF
        || v == 0x2028 || v == 0x2029 || v == 0x00AD
      if hidden {
        out += String(format: "⟦U+%04X⟧", v)
      } else {
        out.unicodeScalars.append(scalar)
      }
    }
    return out
  }

  /// What a fill types, as the extension wrote it (a line break as ⏎, a
  /// literal ⏎ or ⟦ escaped), shown with each ⏎ kept and breaking the line.
  static func typed(_ text: String) -> String {
    escapedControls(text).replacingOccurrences(of: "⏎", with: "⏎\n")
  }

  /// The origin as ASCII: anything else escaped.
  static func ascii(_ origin: String) -> String {
    var out = ""
    for scalar in origin.unicodeScalars {
      if (0x21...0x7E).contains(scalar.value) {
        out.unicodeScalars.append(scalar)
      } else {
        out += String(format: "⟦U+%04X⟧", scalar.value)
      }
    }
    return out
  }
}

/// What a panel is showing.
enum ReviewState: Equatable {
  /// Waiting for the person; its buttons answer from [armedAt].
  case waiting(armedAt: TimeInterval)
  /// Lapsed: says so a moment, buttons dead, then closes.
  case expired
  /// Allowed: the person goes back to the tab to finish; no buttons.
  case allowed
}

/// What the panel does on screen (BrowserReviewPanel.swift), so the desk is
/// checked without AppKit.
protocol ReviewPresenter: AnyObject {
  /// Shows, or updates, helper [id]'s panel.
  func show(_ id: Int, _ content: ReviewContent, _ state: ReviewState, words: ReviewWords)
  /// Updates the time left (whole seconds) on a waiting panel.
  func tick(_ id: Int, secondsLeft: Int)
  /// Closes it.
  func close(_ id: Int)
  /// Brings it in front of other windows, without activating the app.
  func front(_ id: Int) -> Bool
}

/// The reviews on screen, one per helper: what the broker's events open and
/// close, and the only place a decision is made.
final class ReviewDesk {
  /// How long a panel's buttons wait after it appears or its content
  /// changes, so a click meant for something else can't land on them.
  static let armAfter: TimeInterval = 1.0
  /// How long "Expired" shows before the panel closes.
  static let expiredFor: TimeInterval = 1.5
  /// How long, after Allow, the panel says to go back to the tab.
  static let allowedFor: TimeInterval = 12

  var words = ReviewWords()
  weak var presenter: ReviewPresenter?
  /// This Mac's clock, in seconds since 1970 (the extension's expires_ms
  /// counts the same).
  var now: () -> TimeInterval = { Date().timeIntervalSince1970 }

  private let decide: (ReviewClick) -> Bool
  private let stop: (Int) -> Void
  private let decided: (_ helper: Int, _ command: Int, _ allow: Bool) -> Void

  private struct Open {
    var content: ReviewContent
    var state: ReviewState
    /// When it closes by itself (expired or allowed).
    var closesAt: TimeInterval?
  }
  private var open: [Int: Open] = [:]
  /// Who asked and where, per helper, from its binding and hello.
  private var agents: [Int: String] = [:]
  private var origins: [Int: String] = [:]

  /// [decide]: the broker's, the only use of a [ReviewClick]. [stop]: ends
  /// the helper's session (the broker's end). [decided]: tells Dart.
  init(
    decide: @escaping (ReviewClick) -> Bool, stop: @escaping (Int) -> Void,
    decided: @escaping (_ helper: Int, _ command: Int, _ allow: Bool) -> Void
  ) {
    self.decide = decide
    self.stop = stop
    self.decided = decided
  }

  /// Whether any panel is open (the bridge ticks only while one is).
  var isOpen: Bool { !open.isEmpty }

  /// The agent the app bound helper [id] to, from the bound frame it sent.
  func bound(_ id: Int, agent: String) { agents[id] = agent }

  /// A broker event: a verified review opens (or replaces) its helper's
  /// panel; the command's result, a pause, the end or the helper going
  /// closes it.
  func handle(_ event: BrowserBrokerEvent) {
    switch event {
    case .message(let id, let json):
      guard let frame = (try? JSONSerialization.jsonObject(with: json)) as? [String: Any] else { return }
      switch frame["type"] as? String {
      case "hello":
        if origins[id] == nil, let origin = frame["origin"] as? String { origins[id] = origin }
      case "review":
        guard
          let content = ReviewContent.parse(
            frame, agent: agents[id] ?? "", origin: origins[id] ?? "")
        else { return close(id) }
        let entry = Open(content: content, state: .waiting(armedAt: now() + ReviewDesk.armAfter))
        open[id] = entry
        presenter?.show(id, content, entry.state, words: words)
        tick()
      case "result", "paused", "ended":
        close(id)
      default:
        break
      }
    case .gone(let id, _, _):
      close(id)
      agents[id] = nil
      origins[id] = nil
    case .connected, .refused:
      break
    }
  }

  enum Press { case allow, deny, stop }

  /// The person pressed [press] on helper [id]'s panel (or Escape, for
  /// deny). Nothing happens before the panel is armed, once it lapsed, or
  /// when it isn't waiting.
  func press(_ id: Int, _ press: Press) {
    guard let entry = open[id], case .waiting(let armedAt) = entry.state else { return }
    let at = now()
    guard at >= armedAt, at * 1000 < Double(entry.content.expiresMs) else { return }
    if press == .stop {
      close(id)
      stop(id)
      return
    }
    let allow = press == .allow
    let click = ReviewClick(
      helper: id, command: entry.content.command, hash: entry.content.hash, allow: allow)
    guard decide(click) else { return close(id) }
    decided(id, entry.content.command, allow)
    if allow {
      open[id] = Open(content: entry.content, state: .allowed, closesAt: at + ReviewDesk.allowedFor)
      presenter?.show(id, entry.content, .allowed, words: words)
    } else {
      close(id)
    }
  }

  /// Dart asked for helper [id]'s panel: in front, if there's one.
  func bringForward(_ id: Int) -> Bool {
    open[id] == nil ? false : presenter?.front(id) ?? false
  }

  /// Every quarter second while a panel is open: time left, lapsing, and
  /// closing what's had its moment.
  func tick() {
    let at = now()
    for (id, entry) in open {
      if let closesAt = entry.closesAt, at >= closesAt {
        close(id)
        continue
      }
      guard case .waiting = entry.state else { continue }
      let left = Double(entry.content.expiresMs) / 1000 - at
      if left <= 0 {
        open[id] = Open(content: entry.content, state: .expired, closesAt: at + ReviewDesk.expiredFor)
        presenter?.show(id, entry.content, .expired, words: words)
      } else {
        presenter?.tick(id, secondsLeft: Int(left.rounded(.up)))
      }
    }
  }

  /// What helper [id]'s panel is showing, if one is open.
  func state(_ id: Int) -> ReviewState? { open[id]?.state }

  private func close(_ id: Int) {
    guard open.removeValue(forKey: id) != nil else { return }
    presenter?.close(id)
  }
}
