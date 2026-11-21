import Flutter
import UIKit

/// The iPad workbench's commands in the system's menu, on
/// `factory/commands` (app/lib/ipad/commands.dart; the messages are in
/// app/test/ipad/fixtures/commands/): the iPadOS menu bar's View menu and the
/// list shown while ⌘ is held.
///
/// Dart sends `setCommands` whenever they change; this answers true and
/// shows them as UIKeyCommands, so Dart's own key bindings stand down. A
/// command chosen, or its keys pressed, calls back `run` with its id. A
/// disabled one is greyed in the menu and its keys go on to what's focused,
/// as if it weren't there. Anything this can't show as asked (another
/// schema, a modifier it doesn't know) answers false, and Dart keeps the keys.
final class CommandMenu {
  static let channelName = "factory/commands"
  static let shared = CommandMenu()

  /// The menu's identifier, inline at the end of View.
  static let menuIdentifier = UIMenu.Identifier("com.claktory.workbench")

  struct Command: Equatable {
    let id: String
    let title: String
    let input: String
    let modifiers: UIKeyModifierFlags
    let enabled: Bool

    static func == (a: Command, b: Command) -> Bool {
      a.id == b.id && a.title == b.title && a.input == b.input
        && a.modifiers.rawValue == b.modifiers.rawValue && a.enabled == b.enabled
    }
  }

  private(set) var commands: [Command] = []
  private var channel: FlutterMethodChannel?

  /// Asks the system to rebuild its menus; replaced in tests.
  var rebuild: () -> Void = { UIMenuSystem.main.setNeedsRebuild() }

  static func register(with messenger: FlutterBinaryMessenger) {
    shared.attach(FlutterMethodChannel(name: channelName, binaryMessenger: messenger))
  }

  func attach(_ channel: FlutterMethodChannel) {
    self.channel = channel
    channel.setMethodCallHandler { [weak self] call, result in
      guard call.method == "setCommands" else { return result(FlutterMethodNotImplemented) }
      guard let self, let commands = Self.parse(call.arguments) else { return result(false) }
      self.set(commands)
      result(true)
    }
  }

  /// `setCommands`' arguments as commands, or nil when they aren't
  /// schema 1 or a command can't be shown as asked.
  static func parse(_ arguments: Any?) -> [Command]? {
    guard let arguments = arguments as? [String: Any],
      (arguments["schema_version"] as? NSNumber)?.intValue == 1,
      let list = arguments["commands"] as? [[String: Any]]
    else { return nil }
    var commands: [Command] = []
    var ids = Set<String>()
    for entry in list {
      guard let id = entry["id"] as? String, !id.isEmpty, ids.insert(id).inserted,
        let title = entry["title"] as? String, !title.isEmpty,
        let input = entry["input"] as? String, !input.isEmpty,
        let names = entry["modifiers"] as? [String],
        let modifiers = modifierFlags(names),
        let enabled = entry["enabled"] as? Bool
      else { return nil }
      // Esc stays Flutter's: it closes what's open over the window.
      if input == UIKeyCommand.inputEscape { return nil }
      commands.append(
        Command(id: id, title: title, input: input, modifiers: modifiers, enabled: enabled))
    }
    return commands
  }

  /// UIKeyModifierFlags from their names, or nil for one it doesn't know.
  static func modifierFlags(_ names: [String]) -> UIKeyModifierFlags? {
    var flags: UIKeyModifierFlags = []
    for name in names {
      switch name {
      case "command": flags.insert(.command)
      case "control": flags.insert(.control)
      case "alternate": flags.insert(.alternate)
      case "shift": flags.insert(.shift)
      default: return nil
      }
    }
    return flags
  }

  /// Keeps [commands], rebuilding the menu only when what it shows changed
  /// (whether one is enabled is asked each time: [isEnabled]).
  func set(_ commands: [Command]) {
    let shown = { (list: [Command]) in list.map { [$0.id, $0.title, $0.input, "\($0.modifiers.rawValue)"] } }
    let rebuilds = shown(commands) != shown(self.commands)
    self.commands = commands
    if rebuilds { rebuild() }
  }

  func isEnabled(_ id: String?) -> Bool {
    commands.contains { $0.id == id && $0.enabled }
  }

  /// The menu's items, each running [action] with its id as its property
  /// list.
  func keyCommands(action: Selector) -> [UIKeyCommand] {
    commands.map { command in
      let key = UIKeyCommand(
        title: command.title, action: action, input: command.input,
        modifierFlags: command.modifiers, propertyList: command.id)
      // ⌃Tab and the like are the system's focus moves otherwise; the
      // workbench's pane moves come first, as its own keys did.
      if #available(iOS 15.0, *) { key.wantsPriorityOverSystemBehavior = true }
      return key
    }
  }

  /// Adds the commands to the end of the View menu, set apart inline.
  func build(_ builder: UIMenuBuilder, action: Selector) {
    guard builder.system == .main, !commands.isEmpty else { return }
    let menu = UIMenu(
      title: "", identifier: Self.menuIdentifier, options: .displayInline,
      children: keyCommands(action: action))
    builder.insertChild(menu, atEndOfMenu: .view)
  }

  /// Tells Dart the person chose [id].
  func run(_ id: String) {
    guard isEnabled(id) else { return }
    channel?.invokeMethod("run", arguments: ["id": id])
  }
}
