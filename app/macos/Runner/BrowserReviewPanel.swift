// The review panel (B5b): a small floating panel, on every Space, where the
// person allows a browser action once, denies it or stops the session
// (BrowserReview.swift decides what's shown and what a press does). It
// appears without taking focus, in a corner of the pointer's screen away
// from Chrome's window, and takes the keyboard only once clicked. Its
// buttons wait a second after it appears or changes; Return and Space
// never press them, Escape denies. Claktory's look (docs/IDENTITY.md):
// sidebar colour, hairline, 14px radius, Geist, signal for what waits.
import Cocoa
import CoreText

final class BrowserReviewPanels: ReviewPresenter {
  private let desk: ReviewDesk
  private var panels: [Int: ReviewPanel] = [:]

  init(desk: ReviewDesk) {
    self.desk = desk
    ReviewStyle.registerFonts()
  }

  func show(_ id: Int, _ content: ReviewContent, _ state: ReviewState, words: ReviewWords) {
    let panel = panels[id] ?? ReviewPanel { [weak self] press in self?.desk.press(id, press) }
    let isNew = panels[id] == nil
    panels[id] = panel
    panel.render(content, state, words: words, now: desk.now())
    if isNew {
      panel.place()
      panel.orderFrontRegardless()
      NSAccessibility.post(
        element: panel, notification: .announcementRequested,
        userInfo: [
          .announcement: panel.summary, .priority: NSAccessibilityPriorityLevel.high.rawValue,
        ])
    }
  }

  func tick(_ id: Int, secondsLeft: Int) {
    panels[id]?.secondsLeft(secondsLeft)
  }

  func close(_ id: Int) {
    panels.removeValue(forKey: id)?.close()
  }

  func front(_ id: Int) -> Bool {
    guard let panel = panels[id] else { return false }
    panel.orderFrontRegardless()
    return true
  }
}

/// Colours, type and sizes from docs/IDENTITY.md.
enum ReviewStyle {
  static var dark: Bool {
    NSApp.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
  }
  static func color(_ hex: UInt32) -> NSColor {
    NSColor(
      srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
      blue: CGFloat(hex & 0xFF) / 255, alpha: 1)
  }
  static var sidebar: NSColor { color(dark ? 0x151517 : 0xF5F5F6) }
  static var canvas: NSColor { color(dark ? 0x0E0E10 : 0xFFFFFF) }
  static var hairline: NSColor { color(dark ? 0x232327 : 0xE4E4E7) }
  static var text: NSColor { color(dark ? 0xEDEDEF : 0x18181B) }
  static var textMuted: NSColor { color(dark ? 0xA1A1AA : 0x52525B) }
  static var signal: NSColor { color(dark ? 0xFF5B1F : 0xE5480F) }
  static var danger: NSColor { color(dark ? 0xFF5C7A : 0xC8193C) }

  static func font(_ size: CGFloat, _ weight: NSFont.Weight = .regular, mono: Bool = false) -> NSFont {
    let name =
      mono
      ? (weight == .regular ? "GeistMono-Regular" : "GeistMono-Medium")
      : weight == .semibold ? "Geist-SemiBold" : weight == .medium ? "Geist-Medium" : "Geist-Regular"
    return NSFont(name: name, size: size)
      ?? (mono
        ? NSFont.monospacedSystemFont(ofSize: size, weight: weight)
        : NSFont.systemFont(ofSize: size, weight: weight))
  }

  private static var registered = false

  /// Geist and Geist Mono, from the app's own bundled fonts, for this
  /// process only.
  static func registerFonts() {
    guard !registered else { return }
    registered = true
    guard
      let fonts = Bundle.main.privateFrameworksURL?
        .appendingPathComponent("App.framework/Resources/flutter_assets/fonts"),
      let files = try? FileManager.default.contentsOfDirectory(
        at: fonts, includingPropertiesForKeys: nil)
    else { return }
    for file in files where file.lastPathComponent.hasPrefix("Geist") {
      CTFontManagerRegisterFontsForURL(file as CFURL, .process, nil)
    }
  }
}

/// One review's panel.
final class ReviewPanel: NSPanel {
  static let width: CGFloat = 360

  private let pressed: (ReviewDesk.Press) -> Void
  private let stack = NSStackView()
  private let timeLeft = NSTextField(labelWithString: "")
  private var buttons: [ReviewButton] = []
  private var arming: Timer?
  private var words = ReviewWords()
  private(set) var summary = ""

  init(pressed: @escaping (ReviewDesk.Press) -> Void) {
    self.pressed = pressed
    super.init(
      contentRect: NSRect(x: 0, y: 0, width: ReviewPanel.width, height: 200),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    isFloatingPanel = true
    level = .floating
    collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
    hidesOnDeactivate = false
    // Takes the keyboard once clicked, never by appearing.
    becomesKeyOnlyIfNeeded = false
    isReleasedWhenClosed = false
    backgroundColor = .clear
    isOpaque = false
    hasShadow = true
    isMovableByWindowBackground = true
    animationBehavior = .utilityWindow

    let box = NSView()
    box.wantsLayer = true
    box.layer?.cornerRadius = 14
    box.layer?.borderWidth = 1
    box.layer?.masksToBounds = true
    contentView = box
    stack.orientation = .vertical
    stack.alignment = .leading
    stack.spacing = 8
    stack.edgeInsets = NSEdgeInsets(top: 16, left: 16, bottom: 16, right: 16)
    stack.translatesAutoresizingMaskIntoConstraints = false
    box.addSubview(stack)
    NSLayoutConstraint.activate([
      stack.leadingAnchor.constraint(equalTo: box.leadingAnchor),
      stack.trailingAnchor.constraint(equalTo: box.trailingAnchor),
      stack.topAnchor.constraint(equalTo: box.topAnchor),
      stack.bottomAnchor.constraint(equalTo: box.bottomAnchor),
      box.widthAnchor.constraint(equalToConstant: ReviewPanel.width),
    ])
    setAccessibilityRole(.window)
    setAccessibilitySubrole(.floatingWindow)
  }

  override var canBecomeKey: Bool { true }
  override var canBecomeMain: Bool { false }

  /// Escape denies; Return and Space press nothing (no default button, and
  /// the buttons never take focus).
  override func cancelOperation(_ sender: Any?) { pressed(.deny) }
  override func keyDown(with event: NSEvent) {
    if event.keyCode == 53 { return pressed(.deny) }
  }

  func render(_ content: ReviewContent, _ state: ReviewState, words: ReviewWords, now: TimeInterval) {
    self.words = words
    stack.arrangedSubviews.forEach { $0.removeFromSuperview() }
    buttons = []
    arming?.invalidate()
    let box = contentView!
    box.layer?.backgroundColor = ReviewStyle.sidebar.cgColor
    // Signal while it waits for the person, hairline once it doesn't.
    let waiting: Bool = if case .waiting = state { true } else { false }
    box.layer?.borderColor =
      (waiting ? ReviewStyle.signal.withAlphaComponent(0.55) : ReviewStyle.hairline).cgColor

    let titleKey =
      ["click", "fill", "navigate"].contains(content.op) ? "title_\(content.op)" : "title_other"
    let title = words.say(titleKey, ["agent": content.agent, "op": content.op])
    add(label(title, ReviewStyle.font(15, .semibold), ReviewStyle.text))
    // The site on a line of its own, scheme and all, never wrapped: cut in
    // the middle when it's too long, so the host's end stays in view.
    let site = NSTextField(labelWithString: content.origin)
    site.font = ReviewStyle.font(15, .medium)
    site.textColor = ReviewStyle.text
    site.lineBreakMode = .byTruncatingMiddle
    site.maximumNumberOfLines = 1
    site.translatesAutoresizingMaskIntoConstraints = false
    add(site)
    site.widthAnchor.constraint(lessThanOrEqualToConstant: ReviewPanel.width - 32).isActive = true
    site.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
    stack.setCustomSpacing(2, after: stack.arrangedSubviews[stack.arrangedSubviews.count - 2])
    var spoken = [title, content.origin]
    // What it may lead to, first and set apart, never as fine print (as
    // IDENTITY's app access card warns: a danger box).
    // A code without words shows as the code: never hidden.
    func said(_ key: String) -> String {
      words.say(key, ["agent": content.agent, "origin": content.origin])
    }
    var lines = content.warnings.map { (said($0), false) }
    if !content.marks.isEmpty {
      lines.append((said("marks_lead"), false))
      lines += content.marks.map { (said($0), true) }
    }
    if !lines.isEmpty {
      add(warnings(lines))
      spoken += lines.map(\.0)
    }
    for row in content.rows {
      // Where it goes says too when that's another site.
      let value =
        row.key == "destination"
        ? ([row.value] + content.leaves.map { said($0) }).joined(separator: "\n") : row.value
      add(field(words.say(row.key), value))
      spoken.append("\(words.say(row.key)): \(value)")
    }
    if let text = content.text {
      add(label(words.say("text"), ReviewStyle.font(12, .medium), ReviewStyle.textMuted))
      add(typed(text))
      spoken.append("\(words.say("text")): \(text)")
    }
    summary = spoken.joined(separator: ". ")

    switch state {
    case .waiting(let armedAt):
      timeLeft.font = ReviewStyle.font(12)
      timeLeft.textColor = ReviewStyle.textMuted
      secondsLeft(max(0, Int((Double(content.expiresMs) / 1000 - now).rounded(.up))))
      let hint = label(words.say("escape_denies"), ReviewStyle.font(12), ReviewStyle.textMuted)
      let row = NSStackView(views: [timeLeft, hint])
      row.spacing = 8
      add(row)
      let stop = ReviewButton(words.say("stop"), primary: false) { [weak self] in self?.pressed(.stop) }
      let deny = ReviewButton(words.say("deny"), primary: false) { [weak self] in self?.pressed(.deny) }
      let allow = ReviewButton(words.say("allow"), primary: true) { [weak self] in self?.pressed(.allow) }
      buttons = [stop, deny, allow]
      let spacer = NSView()
      spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
      let actions = NSStackView(views: [stop, spacer, deny, allow])
      actions.spacing = 8
      actions.translatesAutoresizingMaskIntoConstraints = false
      add(actions)
      actions.widthAnchor.constraint(equalToConstant: ReviewPanel.width - 32).isActive = true
      // Dead until armed: shown so, and the desk ignores them meanwhile.
      let wait = armedAt - now
      buttons.forEach { $0.armed = wait <= 0 }
      if wait > 0 {
        arming = Timer.scheduledTimer(withTimeInterval: wait, repeats: false) { [weak self] _ in
          self?.buttons.forEach { $0.armed = true }
        }
      }
    case .expired:
      add(label(words.say("expired"), ReviewStyle.font(13, .medium), ReviewStyle.textMuted))
      summary = words.say("expired")
      NSAccessibility.post(
        element: self, notification: .announcementRequested,
        userInfo: [.announcement: summary])
    case .allowed:
      add(label(words.say("go_back"), ReviewStyle.font(13, .medium), ReviewStyle.signal))
      summary = words.say("go_back")
      NSAccessibility.post(
        element: self, notification: .announcementRequested,
        userInfo: [.announcement: summary])
    }
    setAccessibilityLabel("\(title) \(content.origin)")
    box.layoutSubtreeIfNeeded()
    let size = box.fittingSize
    var frame = self.frame
    frame.origin.y += frame.height - size.height
    frame.size = size
    setFrame(frame, display: true)
  }

  func secondsLeft(_ seconds: Int) {
    timeLeft.stringValue = words.say("seconds_left", ["seconds": "\(seconds)"])
    timeLeft.setAccessibilityLabel(timeLeft.stringValue)
  }

  /// A corner of the pointer's screen, below the menu bar, the one that
  /// covers least of Chrome's front window.
  func place() {
    let pointer = NSEvent.mouseLocation
    let screen =
      NSScreen.screens.first { NSMouseInRect(pointer, $0.frame, false) } ?? NSScreen.main
    guard let area = screen?.visibleFrame else { return }
    let size = frame.size
    let margin: CGFloat = 16
    let corners = [
      NSPoint(x: area.maxX - size.width - margin, y: area.maxY - size.height - margin),
      NSPoint(x: area.minX + margin, y: area.maxY - size.height - margin),
      NSPoint(x: area.maxX - size.width - margin, y: area.minY + margin),
      NSPoint(x: area.minX + margin, y: area.minY + margin),
    ]
    let chrome = ReviewPanel.chromeFrame()
    let best =
      corners.min { a, b in
        overlap(NSRect(origin: a, size: size), chrome) < overlap(NSRect(origin: b, size: size), chrome)
      } ?? corners[0]
    setFrameOrigin(best)
  }

  private func overlap(_ a: NSRect, _ b: NSRect?) -> CGFloat {
    guard let b else { return 0 }
    let both = a.intersection(b)
    return both.isNull ? 0 : both.width * both.height
  }

  /// Chrome's frontmost window on screen, in Cocoa's coordinates; nil when
  /// there's none. Bounds only: no window titles or contents are read.
  static func chromeFrame() -> NSRect? {
    guard
      let windows = CGWindowListCopyWindowInfo(
        [.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]],
      let primary = NSScreen.screens.first
    else { return nil }
    for window in windows {
      guard let owner = window[kCGWindowOwnerName as String] as? String,
        owner.hasPrefix("Google Chrome"),
        window[kCGWindowLayer as String] as? Int == 0,
        let bounds = window[kCGWindowBounds as String] as? [String: CGFloat],
        let x = bounds["X"], let y = bounds["Y"], let w = bounds["Width"], let h = bounds["Height"]
      else { continue }
      return NSRect(x: x, y: primary.frame.height - y - h, width: w, height: h)
    }
    return nil
  }

  private func add(_ view: NSView) { stack.addArrangedSubview(view) }

  private func label(_ text: String, _ font: NSFont, _ color: NSColor) -> NSTextField {
    let label = NSTextField(wrappingLabelWithString: text)
    label.font = font
    label.textColor = color
    label.isSelectable = false
    label.preferredMaxLayoutWidth = ReviewPanel.width - 32
    return label
  }

  /// The review's warnings in a box of their own: `danger` at 8%, a
  /// `danger` 40% border, 6px radius, 8px in, each line after a warning
  /// glyph, in body `text` weight 500.
  private func warnings(_ lines: [(String, Bool)]) -> NSView {
    let column = NSStackView()
    column.orientation = .vertical
    column.alignment = .leading
    column.spacing = 4
    column.edgeInsets = NSEdgeInsets(top: 8, left: 8, bottom: 8, right: 8)
    for (line, under) in lines {
      let glyph = NSImageView()
      // A line under a lead line (what a name holds) is indented, no glyph.
      if under {
        glyph.widthAnchor.constraint(equalToConstant: 14).isActive = true
      } else if let image = NSImage(systemSymbolName: "exclamationmark.triangle.fill", accessibilityDescription: nil) {
        glyph.image = image
        glyph.contentTintColor = ReviewStyle.danger
        glyph.symbolConfiguration = .init(pointSize: 12, weight: .medium)
      }
      glyph.setContentHuggingPriority(.required, for: .horizontal)
      let text = label(line, ReviewStyle.font(13, .medium), ReviewStyle.text)
      text.preferredMaxLayoutWidth = ReviewPanel.width - 32 - 16 - 20
      let row = NSStackView(views: [glyph, text])
      row.alignment = .firstBaseline
      row.spacing = 6
      column.addArrangedSubview(row)
      row.widthAnchor.constraint(equalToConstant: ReviewPanel.width - 32 - 16).isActive = true
    }
    column.wantsLayer = true
    column.layer?.cornerRadius = 6
    column.layer?.borderWidth = 1
    column.layer?.borderColor = ReviewStyle.danger.withAlphaComponent(0.4).cgColor
    column.layer?.backgroundColor = ReviewStyle.danger.withAlphaComponent(0.08).cgColor
    column.translatesAutoresizingMaskIntoConstraints = false
    column.widthAnchor.constraint(equalToConstant: ReviewPanel.width - 32).isActive = true
    column.setAccessibilityElement(true)
    column.setAccessibilityRole(.staticText)
    column.setAccessibilityLabel(lines.map(\.0).joined(separator: " "))
    return column
  }

  private func field(_ name: String, _ value: String) -> NSView {
    let column = NSStackView(views: [
      label(name, ReviewStyle.font(12, .medium), ReviewStyle.textMuted),
      label(value, ReviewStyle.font(13), ReviewStyle.text),
    ])
    column.orientation = .vertical
    column.alignment = .leading
    column.spacing = 2
    column.setAccessibilityElement(true)
    column.setAccessibilityRole(.staticText)
    column.setAccessibilityLabel("\(name): \(value)")
    return column
  }

  /// What a fill types, whole, scrolling past six lines: on the canvas,
  /// behind a hairline, in Geist Mono.
  private func typed(_ text: String) -> NSView {
    let scroll = NSTextView.scrollableTextView()
    let view = scroll.documentView as! NSTextView
    view.string = text
    view.isEditable = false
    view.isSelectable = false
    view.font = ReviewStyle.font(12, mono: true)
    view.textColor = ReviewStyle.text
    view.backgroundColor = ReviewStyle.canvas
    view.textContainerInset = NSSize(width: 8, height: 6)
    scroll.drawsBackground = false
    scroll.wantsLayer = true
    scroll.layer?.cornerRadius = 6
    scroll.layer?.borderWidth = 1
    scroll.layer?.borderColor = ReviewStyle.hairline.cgColor
    scroll.hasVerticalScroller = true
    scroll.translatesAutoresizingMaskIntoConstraints = false
    // Laid out at its real width first, so its height is the text's.
    let width = ReviewPanel.width - 32
    scroll.frame = NSRect(x: 0, y: 0, width: width, height: 400)
    view.frame = NSRect(x: 0, y: 0, width: scroll.contentSize.width, height: 400)
    view.textContainer?.containerSize = NSSize(
      width: scroll.contentSize.width - 16, height: .greatestFiniteMagnitude)
    view.layoutManager?.ensureLayout(for: view.textContainer!)
    let lines = (view.layoutManager?.usedRect(for: view.textContainer!).height ?? 16)
    let height = min(lines + 12, 8 * 15 + 12)
    view.scroll(.zero)
    NSLayoutConstraint.activate([
      scroll.widthAnchor.constraint(equalToConstant: ReviewPanel.width - 32),
      scroll.heightAnchor.constraint(equalToConstant: max(height, 28)),
    ])
    return scroll
  }
}

/// A panel button: quiet, or primary on the signal colour with black text.
/// It never takes focus (so Space can't press it) and has no key
/// equivalent (so Return can't).
final class ReviewButton: NSButton {
  private let action_: () -> Void
  private let primary: Bool

  var armed = false {
    didSet { alphaValue = armed ? 1 : 0.4 }
  }

  init(_ title: String, primary: Bool, action: @escaping () -> Void) {
    self.action_ = action
    self.primary = primary
    super.init(frame: .zero)
    isBordered = false
    wantsLayer = true
    layer?.cornerRadius = 6
    layer?.backgroundColor = (primary ? ReviewStyle.signal : ReviewStyle.canvas).cgColor
    if !primary {
      layer?.borderWidth = 1
      layer?.borderColor = ReviewStyle.hairline.cgColor
    }
    attributedTitle = NSAttributedString(
      string: title,
      attributes: [
        .font: ReviewStyle.font(13, .medium),
        .foregroundColor: primary ? NSColor.black : ReviewStyle.text,
      ])
    keyEquivalent = ""
    refusesFirstResponder = true
    target = self
    self.action = #selector(clicked)
    translatesAutoresizingMaskIntoConstraints = false
    heightAnchor.constraint(equalToConstant: 28).isActive = true
    widthAnchor.constraint(greaterThanOrEqualToConstant: 72).isActive = true
    alphaValue = 0.4
    setAccessibilityLabel(title)
  }

  required init?(coder: NSCoder) { fatalError() }

  override var acceptsFirstResponder: Bool { false }
  /// A click on a panel that isn't key still presses (it's the person's).
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

  @objc private func clicked() { action_() }
}
