import 'dart:async';

import 'package:flutter/widgets.dart';

import '../app_grant_card.dart' show appGrantUntil;
import '../characters/character.dart';
import '../controls.dart';
import '../device_account.dart' show AccountClaktory;
import '../forms.dart';
import '../identity.dart';
import '../message_time.dart' show dayLine;
import '../mobile/metrics.dart';
import '../people/contacts.dart';
import '../people/e2ee.dart' show e2eeOf;
import '../people/people_view.dart' show contactsApi;
import '../people/problems.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../scope.dart';
import 'grants.dart';
import 'session.dart';

// U3b: Agents' access, the grants agents on the person's Claktories hold
// in their account-level apps (direct messages first), straight from the
// service, with Remove. Each says plainly who (the agent, named by the
// shown Claktory when it's there, and its id), on which Claktory, in which
// app, what it may do and where: the friends' conversations by their
// names and tags, never ids. Nothing opens it yet: it ships with DMs.

/// Opens Agents' access.
Future<void> showAgentsAccess(BuildContext context) =>
    showModal(context, (context, close) => AgentsAccessView(close: close));

/// Stand in for the account service in tests.
GrantsApi Function(BuildContext context)? grantsApi;
Future<List<AccountClaktory>> Function(BuildContext context)? accountClaktories;

/// Everything a row needs, read at once.
class _Access {
  const _Access({
    required this.grants,
    required this.claktories,
    required this.conversations,
  });

  final List<UserAppGrant> grants;

  /// The account's Claktories by id; null when they couldn't be read.
  final Map<String, AccountClaktory>? claktories;

  /// A friend's conversation id to who they are ("Ana · K7QF2M"); null
  /// when the friends couldn't be read.
  final Map<String, String>? conversations;
}

/// Who a grant reaches in direct messages: the friends by name, those it
/// can't name counted after them, "In conversations with Ana · K7QF2M and
/// 1 other"; never an id. Counted when none can be named.
String grantReach(
  List<({String kind, String id})> resources,
  Map<String, String>? conversations,
) {
  if (resources.isEmpty) return '';
  final chats = [
    for (final resource in resources)
      if (resource.kind == 'conversation') resource.id,
  ];
  final others = resources.length - chats.length;
  final parts = <String>[];
  if (chats.isNotEmpty) {
    final named = conversations == null
        ? const <String>[]
        : [for (final id in chats) ?conversations[id]];
    final rest = chats.length - named.length;
    if (named.isEmpty) {
      parts.add('${chats.length} conversation${chats.length == 1 ? '' : 's'}');
    } else {
      // "Ana and Bob", or "Ana, Bob and 1 other".
      parts.add(
        'conversations with '
        '${rest > 0 ? named.join(', ') : _listed(named)}',
      );
      if (rest > 0) parts.add('$rest other${rest == 1 ? '' : 's'}');
    }
  }
  if (others > 0) parts.add('$others other place${others == 1 ? '' : 's'}');
  final words = _listed(parts);
  return 'In $words';
}

String _listed(List<String> items) => items.length < 2
    ? items.join()
    : '${items.sublist(0, items.length - 1).join(', ')} and ${items.last}';

class AgentsAccessView extends StatefulWidget {
  const AgentsAccessView({super.key, required this.close});

  final VoidCallback close;

  @override
  State<AgentsAccessView> createState() => _AgentsAccessViewState();
}

class _AgentsAccessViewState extends State<AgentsAccessView> {
  late final GrantsApi _grants =
      grantsApi?.call(context) ?? GrantsApi(UserAppsSession.of(context));
  late final ContactsApi _contacts =
      contactsApi?.call(context) ?? ContactsApi(UserAppsSession.of(context));
  _Access? _access;
  String? _error;
  var _busy = false;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  /// The grants, and what names them: the account's Claktories and the
  /// person's friends, each optional (a row says less without them).
  Future<void> _load() async {
    final claktories =
        (accountClaktories?.call(context) ??
                AppScope.scope(context).connections.account.claktories())
            .then<Map<String, AccountClaktory>?>(
              (list) => {for (final claktory in list) claktory.id: claktory},
              onError: (Object _) => null,
            );
    final friends = _contacts.lists().then<Map<String, String>?>(
      (lists) => {
        for (final friend in lists.friends)
          if (friend.conversationId.isNotEmpty)
            friend.conversationId: friend.person.label,
      },
      onError: (Object _) => null,
    );
    try {
      final grants = await _grants.list();
      final access = _Access(
        grants: grants,
        claktories: await claktories,
        conversations: await friends,
      );
      if (mounted) setState(() => _access = access);
    } on UserAppError catch (error) {
      if (mounted) setState(() => _error = _problem(error));
    }
  }

  String _problem(UserAppError error) => switch (error.problem) {
    UserAppProblem.notSignedIn =>
      'Sign in to your Claktory account on this device to see which '
          'agents can use your apps.',
    UserAppProblem.notFound =>
      'That access was already taken back. What you see is up to date now.',
    _ => problemText(error),
  };

  Future<void> _remove(UserAppGrant grant, String agent) async {
    var sure = false;
    final app = userAppName(grant.app);
    await showModal(
      context,
      (context, close) => FormFrame(
        title: "Remove $agent's access to $app?",
        body:
            "$agent can't use $app from now on. To give access again, "
            'Rivet has to suggest it and you approve it.',
        fields: const [],
        error: null,
        submitLabel: 'Remove access',
        onSubmit: () {
          sure = true;
          close();
        },
        onCancel: close,
      ),
    );
    if (!sure || !mounted) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    String? error;
    try {
      await _grants.revoke(grant.id);
      // Its Claktory comes out of those conversations' groups now, from
      // this device when it's set up for messages (Fugo L6).
      if (grant.app == 'dms' && mounted) {
        unawaited(e2eeOf(context).grantsChanged());
      }
    } on UserAppError catch (failure) {
      error = _problem(failure);
    }
    await _load();
    if (mounted) {
      setState(() {
        _busy = false;
        if (error != null) _error = error;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final access = _access;
    final workspace = AppScope.of(context).workspace.workspace;
    return FormFrame(
      title: "Agents' access",
      body:
          'What your agents can use in your Claktory account, such as your '
          'direct messages. Removing access works at once, even while a '
          'Claktory is off.',
      fields: [
        if (access == null && _error == null)
          Text(
            "Getting your agents' access…",
            style: type.body.copyWith(color: palette.textMuted),
          ),
        if (access != null && access.grants.isEmpty)
          Text(
            'No agent can use your apps.',
            style: type.body.copyWith(color: palette.textMuted),
          ),
        if (access != null)
          for (final grant in access.grants)
            _GrantRow(
              key: ValueKey(grant.id),
              grant: grant,
              access: access,
              workspace: workspace,
              busy: _busy,
              onRemove: _remove,
            ),
        const SizedBox(height: Space.s),
      ],
      error: _error,
      submitLabel: 'Done',
      onSubmit: widget.close,
      onCancel: widget.close,
    );
  }
}

/// One grant: the agent (its character and name when the shown Claktory
/// has it, with its id after, faint), on which Claktory, in which app and
/// since when; what it may do, where, until when; and Remove.
class _GrantRow extends StatelessWidget {
  const _GrantRow({
    super.key,
    required this.grant,
    required this.access,
    required this.workspace,
    required this.busy,
    required this.onRemove,
  });

  final UserAppGrant grant;
  final _Access access;
  final Workspace workspace;
  final bool busy;
  final void Function(UserAppGrant grant, String agent) onRemove;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final claktory = access.claktories?[grant.claktoryId];
    // The shown Claktory names its own agents.
    final here =
        claktory != null &&
        claktory.endpointId.isNotEmpty &&
        claktory.endpointId == workspace.remote.endpointId.toLowerCase();
    final agent = here
        ? workspace.agents.where((a) => a.id == grant.agentId).firstOrNull
        : null;
    final name = agent?.name ?? grant.agentId;
    final where = claktory != null
        ? 'On ${claktory.name.isEmpty ? claktory.machine : claktory.name}'
        : access.claktories == null
        ? 'On one of your Claktories'
        : "On a Claktory that's no longer on your account";
    final since = grant.grantedMs > 0
        ? ' · since ${_dayIn(grant.grantedMs, DateTime.now())}'
        : '';
    final reach = grantReach(grant.resources, access.conversations);
    final caption = type.caption.copyWith(color: palette.textMuted);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: Space.s),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 28,
            child: agent == null
                ? null
                : AgentCharacter(agent: agent, size: 24),
          ),
          const SizedBox(width: Space.m),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text.rich(
                  TextSpan(
                    children: [
                      TextSpan(
                        text: name,
                        style: TextStyle(color: palette.text),
                      ),
                      // Which agent exactly: its id, as a tag.
                      if (agent != null && agent.name != grant.agentId)
                        TextSpan(
                          text: ' · ${grant.agentId}',
                          style: TextStyle(color: palette.textFaint),
                        ),
                    ],
                  ),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: type.body,
                ),
                Text(
                  '$where · ${userAppName(grant.app)}$since',
                  style: caption,
                ),
                const SizedBox(height: Space.xs),
                for (final scope in grant.scopes)
                  Text(
                    '•  ${userAppScopeLabel(scope)}',
                    style: type.caption.copyWith(color: palette.text),
                  ),
                if (reach.isNotEmpty)
                  Text(
                    reach,
                    style: type.caption.copyWith(color: palette.text),
                  ),
                Text(
                  appGrantUntil(grant.expiresMs),
                  style: type.caption.copyWith(color: palette.textFaint),
                ),
              ],
            ),
          ),
          const SizedBox(width: Space.s),
          Button(
            label: 'Remove…',
            onPressed: busy ? null : () => onRemove(grant, name),
          ),
        ],
      ),
    );
  }
}

String _dayIn(int ms, DateTime now) {
  final day = dayLine(ms, now);
  return day == 'Today' || day == 'Yesterday' ? day.toLowerCase() : day;
}
