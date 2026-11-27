import 'dart:async';
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../backend.dart';
import '../protocol/factory/v1/factory.pb.dart';

// This device's side of Claktory's browser (#435 P5). The person never
// drives it (Matt, card 222): agents run errands in it, and the person
// sees how each goes, its log, and can stop it. What it shows isn't
// streamed here.

/// Whether this backend has Claktory's browser at all.
bool agentBrowserAvailable(Workspace workspace) => workspace.hasAgentBrowser();

/// Whether the browser is running, or on its way.
bool agentBrowserRunning(AgentBrowserState state) => switch (state) {
  AgentBrowserState.AGENT_BROWSER_STATE_STARTING ||
  AgentBrowserState.AGENT_BROWSER_STATE_WARM ||
  AgentBrowserState.AGENT_BROWSER_STATE_DRIVEN ||
  AgentBrowserState.AGENT_BROWSER_STATE_PERSONS_TURN ||
  AgentBrowserState.AGENT_BROWSER_STATE_TAKEN_OVER ||
  AgentBrowserState.AGENT_BROWSER_STATE_BROWSING => true,
  _ => false,
};

/// The errand using the browser now, if any.
Errand? currentErrand(Workspace workspace) {
  final id = workspace.agentBrowser.errandId;
  if (id.isEmpty) return null;
  for (final errand in workspace.errands) {
    if (errand.id == id) return errand;
  }
  return null;
}

/// [agentId]'s errand that hasn't ended, if any.
Errand? runningErrandOf(Workspace workspace, String agentId) {
  for (final errand in workspace.errands) {
    if (errand.agentId == agentId && errandOpen(errand.state)) return errand;
  }
  return null;
}

/// An errand still going: its agent works, or it waits for the person.
bool errandOpen(ErrandState state) => switch (state) {
  ErrandState.ERRAND_STATE_RUNNING ||
  ErrandState.ERRAND_STATE_PERSONS_TURN ||
  ErrandState.ERRAND_STATE_TAKEN_OVER => true,
  _ => false,
};

/// This device's time zone, by IANA name ("Europe/London"), from the native
/// side; null where it doesn't say (an older app, a platform without it).
Future<String?> deviceTimeZone() async {
  try {
    final zone = await const MethodChannel('factory/device')
        .invokeMethod<String>('timeZone');
    return zone == null || zone.isEmpty ? null : zone;
  } on Object {
    return null;
  }
}

/// This device's languages, most preferred first, as BCP 47 tags ("en-GB"),
/// at most 8.
List<String> deviceLanguages() {
  final tags = <String>[];
  for (final locale in ui.PlatformDispatcher.instance.locales) {
    final tag = locale.toLanguageTag();
    if (tag != 'und' && !tags.contains(tag)) tags.add(tag);
  }
  return tags.take(8).toList();
}

/// What the person does with Claktory's browser, for one Claktory: stop
/// it, delete what it keeps, set its time zone and languages.
class AgentBrowserSession extends ChangeNotifier {
  AgentBrowserSession({
    required this.send,
    this.timeZone = deviceTimeZone,
    this.languages = deviceLanguages,
  });

  /// The one for [backend], made the first time it's asked for.
  factory AgentBrowserSession.of(Backend backend) => _sessions[backend] ??=
      AgentBrowserSession(send: backend.sendAgentBrowser);

  static final _sessions = Expando<AgentBrowserSession>();

  /// Uses [session] for [backend] (tests).
  @visibleForTesting
  static void attach(Backend backend, AgentBrowserSession session) =>
      _sessions[backend] = session;

  final Future<void> Function(ClientMessage message) send;

  /// This device's time zone and languages, for Use this device's.
  final Future<String?> Function() timeZone;
  final List<String> Function() languages;

  /// Why the last Stop failed, in Claktory's words; null after one that
  /// worked.
  String? get error => _error;
  String? _error;

  /// Stops it, and the errand using it. Its sign-ins are kept.
  void stop() => unawaited(
    send(ClientMessage(stopAgentBrowser: StopAgentBrowser())).then(
      (_) {
        if (_error == null) return;
        _error = null;
        notifyListeners();
      },
      onError: (Object error) {
        _error = error is RequestFailure ? error.message : '$error';
        notifyListeners();
      },
    ),
  );

  /// Sets its time zone and languages, both at once; empty is not set (the
  /// computer's zone, en-US). They apply from its next start.
  Future<void> setLocale(String timeZone, List<String> languages) => send(
    ClientMessage(
      setAgentBrowserLocale: SetAgentBrowserLocale(
        timeZone: timeZone,
        languages: languages,
      ),
    ),
  );

  /// Deletes everything it keeps. The app asks first.
  Future<void> deleteData() =>
      send(ClientMessage(deleteAgentBrowserData: DeleteAgentBrowserData()));

  /// Signs it out of one identity provider (AgentBrowserAccount.provider):
  /// at once, or once the errand using it ends. The app asks first.
  Future<void> signOut(String provider) => send(
    ClientMessage(
      signOutAgentBrowserAccount: SignOutAgentBrowserAccount(
        provider: provider,
      ),
    ),
  );
}
