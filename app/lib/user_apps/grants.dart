import 'session.dart';

// U3b: the grants agents hold in the person's account-level apps, straight
// from the service (GET and DELETE /v1/apps/grants, device signed), so they
// can be read and taken back with every Claktory off.

/// An app kept at the account, as Claktory calls it: "Direct messages".
String userAppName(String app) => switch (app) {
  'dms' => 'Direct messages',
  _ => app,
};

/// What a scope lets an agent do, in the words Rivet's card uses
/// (factoryd's user_apps): "Read these conversations".
String userAppScopeLabel(String scope) => switch (scope) {
  'dm.read' => 'Read these conversations',
  'dm.draft' => 'Write drafts in them for you to send',
  'dm.send' => 'Send messages as you in them',
  'dm.subscribe' => 'Know when a new message arrives in them',
  _ => scope,
};

/// One grant: one agent of one Claktory may use [scopes] of [app], in
/// [resources], from [grantedMs] until [expiresMs] (0: until taken back).
class UserAppGrant {
  const UserAppGrant({
    required this.id,
    required this.app,
    required this.claktoryId,
    required this.agentId,
    required this.incarnation,
    required this.scopes,
    required this.resources,
    required this.grantedMs,
    required this.expiresMs,
  });

  factory UserAppGrant.fromJson(Map<String, Object?> json) {
    final agent = switch (json['agent']) {
      final Map<String, Object?> agent => agent,
      _ => const <String, Object?>{},
    };
    return UserAppGrant(
      id: _text(json, 'id'),
      app: _text(json, 'app'),
      claktoryId: _text(json, 'claktory_id'),
      agentId: _text(agent, 'id'),
      incarnation: _text(agent, 'incarnation'),
      scopes: [
        for (final scope in json['scopes'] as List? ?? const [])
          if (scope is String) scope,
      ],
      resources: [
        for (final resource in json['resources'] as List? ?? const [])
          if (resource is Map<String, Object?>)
            (kind: _text(resource, 'kind'), id: _text(resource, 'id')),
      ],
      grantedMs: _int(json, 'granted_ms'),
      expiresMs: _int(json, 'expires_ms'),
    );
  }

  final String id;
  final String app;
  final String claktoryId;
  final String agentId;
  final String incarnation;
  final List<String> scopes;
  final List<({String kind, String id})> resources;
  final int grantedMs;
  final int expiresMs;
}

/// The grants routes, through [session].
class GrantsApi {
  const GrantsApi(this.session);

  final UserAppsSession session;

  /// The account's live grants, across its Claktories.
  Future<List<UserAppGrant>> list() async {
    final answer = await session.get('/v1/apps/grants');
    return [
      for (final item in answer['grants'] as List? ?? const [])
        if (item is Map<String, Object?>) UserAppGrant.fromJson(item),
    ];
  }

  /// Takes [grantId] back, at once, wherever the agent is.
  Future<void> revoke(String grantId) =>
      session.delete('/v1/apps/grants/${Uri.encodeComponent(grantId)}');
}

String _text(Map<String, Object?> json, String key) => switch (json[key]) {
  final String text => text,
  _ => '',
};

int _int(Map<String, Object?> json, String key) => switch (json[key]) {
  final int number => number,
  _ => 0,
};
