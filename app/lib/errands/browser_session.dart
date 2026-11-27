import 'dart:async';
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../backend.dart';
import '../protocol/factory/v1/factory.pb.dart';

/// Whether this backend has Claktory's browser at all.
bool agentBrowserAvailable(Workspace workspace) => workspace.hasAgentBrowser();

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
class AgentBrowserSession {
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

  Future<void> deleteData({required String agentId}) => send(
    ClientMessage(
      deleteAgentBrowserData: DeleteAgentBrowserData(agentId: agentId),
    ),
  );

  Future<void> signOut({required String agentId, required String site}) => send(
    ClientMessage(
      signOutAgentBrowserAccount: SignOutAgentBrowserAccount(
        agentId: agentId,
        site: site,
      ),
    ),
  );
}
