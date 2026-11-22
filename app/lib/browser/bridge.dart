import 'dart:convert';

import 'package:flutter/services.dart';

// B5a: the Mac app's side of browser control, over Pesci's broker
// (app/macos/Runner/BrowserBridge.swift, channel factory/browser). The
// broker listens for Claktory's signed helper, which Chrome starts for
// Claktory's extension; each helper that passes the signature check is a
// tab the person may choose, and nothing reaches it until they do.
//
// Off until [BrowserBridge.start], which runs only while browser control
// is on for this Mac: off for everyone until it passes its security proof.

/// Why the broker couldn't start, in the person's words, by its code.
String browserStartProblem(String code) => switch (code) {
  'unsigned' =>
    'Browser control works only in the Claktory app from '
        'claktory.com.',
  'dev_no_helper' =>
    "This build of Claktory Dev doesn't include browser control.",
  'path_too_long' =>
    "Browser control can't start because your Mac's user "
        'folder name is too long.',
  'folder_unsafe' =>
    "Browser control can't start: a private folder it needs "
        "isn't safe. Quit Claktory and open it again.",
  _ => "Browser control can't start.",
};

/// Why a tab's connection ended, after "Disconnected: ", by its code.
/// Anything this app doesn't know reads as the unexpected.
String browserGoneReason(String code) => switch (code) {
  'closed' => 'Chrome or the tab closed the connection.',
  'version' =>
    'Claktory and its Chrome extension are different versions. '
        'Update both.',
  'not_chosen' =>
    "You didn't confirm this tab in Claktory within 5 minutes. "
        'Click the Claktory button in Chrome to try again.',
  'stopped' || 'stop' => 'Browser control stopped on this Mac.',
  _ => "Claktory got something it didn't expect from Chrome.",
};

/// The native review panel's fixed labels (BrowserReviewPanel.swift's
/// keys), Trish's words. What it shows of the action is the extension's,
/// as the helper verified it, never these.
const browserReviewWords = {
  'title_click': '{agent} wants to click on',
  'title_fill': '{agent} wants to type on',
  'title_navigate': '{agent} wants to open a page on',
  'title_other': '{agent} wants to do something on',
  'page': 'Page',
  'target': 'On',
  'shows': 'Shows',
  'text': 'Types',
  'destination': 'Goes to',
  'leaves_other_origin': 'Goes to another site',
  'warning_may_send_as_typed': "The page may send this as it's typed.",
  'warning_may_ask_permission':
      'The page may then ask for a permission, such as the camera.',
  'warning_submits_form': 'This submits a form.',
  'warning_ends_session': "This leaves the site, which ends {agent}'s access.",
  'marks_lead': 'Its name may not be what it looks like:',
  'mark_bidi_control': "It has characters that change the text's direction.",
  'mark_zero_width': 'It has invisible characters.',
  'mark_mixed_script': 'It mixes look-alike letters from different alphabets.',
  'mark_line_break': 'It has a line break.',
  'allow': 'Allow once',
  'deny': "Don't allow",
  'stop': 'Stop',
  'seconds_left': '{seconds} s left',
  'escape_denies': "Esc: Don't allow",
  'expired': 'Expired',
  'go_back': 'Go back to the tab in Chrome within 10 seconds to finish.',
  'unshown_details':
      "Some of what it will do can't be shown here. Update Claktory to see "
      "it, and allow it only if you're sure.",
};

/// This app's release version, for the broker to check the helper's
/// against (only when both report one); "dev" when unstamped.
const _appVersion = String.fromEnvironment(
  'FACTORY_VERSION',
  defaultValue: 'dev',
);

/// When something that isn't Claktory's helper tried to connect.
const browserRefusedLine =
    "Something that isn't Claktory's Chrome extension tried to connect, so "
    'Claktory turned it away.';

/// The broker couldn't start: [code] (see [browserStartProblem]) and its
/// technical [detail].
class BrowserStartFailure implements Exception {
  const BrowserStartFailure(this.code, this.detail);

  final String code;
  final String detail;

  String get message => browserStartProblem(code);

  @override
  String toString() => 'BrowserStartFailure($code: $detail)';
}

/// Chrome's note on where Claktory's helper is (its native messaging
/// manifest) couldn't be written or removed: [detail] is the helper's
/// reason, kept on this Mac.
class BrowserRegisterFailure implements Exception {
  const BrowserRegisterFailure(this.detail);

  final String detail;

  /// Another program's file is where Claktory's would go.
  bool get otherProgram => detail.contains('belongs to another program');

  String get message => otherProgram
      ? "Browser control can't connect to Chrome: another app is using the "
            'connection Claktory needs.'
      : "Browser control can't connect to Chrome: Claktory couldn't set up "
            'its connection.';

  @override
  String toString() => 'BrowserRegisterFailure($detail)';
}

/// What the broker tells the app.
sealed class BrowserBrokerEvent {
  const BrowserBrokerEvent();
}

/// A helper passed the signature check. [code] is the six pairing digits
/// the app sent it, which Chrome's popup shows too.
class HelperConnected extends BrowserBrokerEvent {
  const HelperConnected({
    required this.id,
    required this.version,
    required this.caller,
    required this.code,
  });

  final int id;
  final int version;
  final String caller;
  final String code;
}

/// One frame from Chrome, through helper [id], as it came: a JSON object.
class HelperMessage extends BrowserBrokerEvent {
  const HelperMessage(this.id, this.frame);

  final int id;
  final Map<String, Object?> frame;
}

/// Helper [id] is gone: [code] (see [browserGoneReason]), [detail] faint.
class HelperGone extends BrowserBrokerEvent {
  const HelperGone(this.id, this.code, this.detail);

  final int id;
  final String code;
  final String detail;
}

/// The person decided a review in the native panel on this Mac (B5b):
/// only that panel's buttons can, never this app. Stop there ends the
/// helper instead ([HelperGone], code stop).
class HelperDecided extends BrowserBrokerEvent {
  const HelperDecided(this.id, this.command, {required this.allow});

  final int id;
  final int command;
  final bool allow;
}

/// A peer that failed the check; it sent nothing on.
class HelperRefused extends BrowserBrokerEvent {
  const HelperRefused(this.code, this.detail);

  final String code;
  final String detail;
}

/// The factory/browser channel.
class BrowserBridge {
  BrowserBridge({MethodChannel? channel})
    : _channel = channel ?? const MethodChannel('factory/browser');

  final MethodChannel _channel;

  /// Listens to the broker; [onEvent] gets each event. Frames that aren't
  /// JSON objects never reach it (the broker checks that too).
  void listen(void Function(BrowserBrokerEvent event) onEvent) {
    _channel.setMethodCallHandler((call) async {
      final args = switch (call.arguments) {
        final Map<Object?, Object?> map => map,
        _ => const <Object?, Object?>{},
      };
      int id() => switch (args['id']) {
        final int id => id,
        _ => -1,
      };
      String text(String key) => switch (args[key]) {
        final String value => value,
        _ => '',
      };
      final event = switch (call.method) {
        'helperConnected' => HelperConnected(
          id: id(),
          version: switch (args['version']) {
            final int version => version,
            _ => 0,
          },
          caller: text('caller'),
          code: text('code'),
        ),
        'helperMessage' => switch (_object(text('json'))) {
          final frame? => HelperMessage(id(), frame),
          null => HelperGone(id(), 'unexpected', 'not a JSON object'),
        },
        'helperGone' => HelperGone(id(), text('code'), text('detail')),
        'refused' => HelperRefused(text('code'), text('detail')),
        'decided' => HelperDecided(id(), switch (args['command']) {
          final int command => command,
          _ => -1,
        }, allow: args['allow'] == true),
        _ => null,
      };
      if (event != null) onEvent(event);
    });
  }

  /// Starts listening for helpers: the socket's path. Throws
  /// [BrowserStartFailure].
  Future<String> start() async {
    try {
      final answer = await _channel.invokeMapMethod<String, Object?>('start', {
        'version': _appVersion,
        'words': browserReviewWords,
      });
      return answer?['path'] as String? ?? '';
    } on PlatformException catch (error) {
      throw BrowserStartFailure(error.code, error.message ?? '');
    }
  }

  /// Stops listening; every helper goes.
  Future<void> stop() => _channel.invokeMethod('stop');

  /// Brings helper [id]'s review panel in front, without activating the
  /// app; false when none is open.
  Future<bool> showReview(int id) async =>
      await _channel.invokeMethod<bool>('showReview', {'id': id}) ?? false;

  /// Whether Chrome knows where Claktory's helper is, for this app.
  Future<bool> registered() async =>
      await _channel.invokeMethod<bool>('registered') ?? false;

  /// Tells Chrome where Claktory's helper is, so its extension can start
  /// it; [replace] only once the person chose to replace another
  /// program's file. Throws [BrowserRegisterFailure].
  Future<void> register({bool replace = false}) =>
      _registering('register', {'replace': replace});

  /// Takes that back, when browser control is turned off here.
  Future<void> unregister() => _registering('unregister');

  Future<void> _registering(String method, [Object? arguments]) async {
    try {
      await _channel.invokeMethod<Object?>(method, arguments);
    } on PlatformException catch (error) {
      throw BrowserRegisterFailure(error.message ?? '');
    }
  }

  /// Sends [frame] to helper [id]; false when it's gone (or too big).
  Future<bool> send(int id, Map<String, Object?> frame) async =>
      await _channel.invokeMethod<bool>('send', {
        'id': id,
        'json': jsonEncode(frame),
      }) ??
      false;

  /// The person chose helper [id] here: it stays past its 5 minutes.
  Future<bool> choose(int id) async =>
      await _channel.invokeMethod<bool>('choose', {'id': id}) ?? false;

  /// Ends helper [id] here (it tells Chrome to stop), for [code].
  Future<void> end(int id, String code) =>
      _channel.invokeMethod('end', {'id': id, 'code': code});
}

Map<String, Object?>? _object(String json) {
  try {
    return switch (jsonDecode(json)) {
      final Map<String, Object?> map => map,
      _ => null,
    };
  } on FormatException {
    return null;
  }
}
