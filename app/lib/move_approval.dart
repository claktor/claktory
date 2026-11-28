// Approving a move with the account (the account service's API.md, "Moves
// with an account"): another computer ran `factoryd receive --email`
// (install.sh --receive --email), and the account service holds its
// request until the person approves it. Never pushed: no bell, Needs you
// item or notification. The person finds it here,
// in Settings → Move, on a device signed in to the account and connected to
// the Claktory that moves, while this screen is open.
//
// The card leads with what the service saw of that computer (its address,
// network and place), then what it calls itself, labelled as its own words,
// and its key's fingerprint, small. Approve has this Claktory vouch for this
// device (ProveAllowedDevice), then the device sign the move itself after its
// owner authenticates (device_approval.dart, Prosciutto's), with an expiry no
// later than the request's end; the source moves only on that signature,
// checked against its own devices, never on the service's word.
import 'dart:async';
import 'dart:convert';

import 'package:crypto/crypto.dart' show sha256;
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'claktory_identity.dart' show claktoryNameInText;
import 'controls.dart';
import 'device_account.dart'
    show AccountServiceError, AccountServiceUnreachable, DeviceAccount, b64;
import 'device_approval.dart';
import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;
import 'scope.dart';
import 'visible_codes.dart' show visibleCodes;

/// The short fingerprint of the receive key [endpoint] (64 hex), made here
/// from the key the approval names, as claktory-account's key_fingerprint
/// makes it from the key's base64url: the first 16 hex of its SHA-256. The
/// one fact on the card a service can't choose for another computer (Fugo
/// 51031). Null for an endpoint that isn't 32 bytes of hex.
String? endpointFingerprint(String endpoint) {
  if (!RegExp(r'^[0-9a-fA-F]{64}$').hasMatch(endpoint)) return null;
  final key = [
    for (var i = 0; i < 64; i += 2)
      int.parse(endpoint.substring(i, i + 2), radix: 16),
  ];
  return sha256.convert(utf8.encode(b64(key))).toString().substring(0, 16);
}

/// [fingerprint] as the card and `factoryd receive --email` show it, to
/// compare by eye: "ABCD EF01 2345 6789".
String fingerprintGroups(String fingerprint) {
  final upper = fingerprint.toUpperCase();
  return [
    for (var i = 0; i < upper.length; i += 4)
      upper.substring(i, i + 4 > upper.length ? upper.length : i + 4),
  ].join(' ');
}

/// How long a request waits for the person, from its start (API.md: "for
/// 10 minutes"): its end is `started` plus this.
const moveRequestLifetime = 600;

/// A move waiting for the person's approval (GET /v1/devices/moves), as it
/// was when fetched.
class WaitingMove {
  WaitingMove({
    required this.id,
    this.state = 'waiting',
    required this.started,
    required this.expiresIn,
    this.ip = '',
    this.network = '',
    this.place = '',
    this.name = '',
    this.platform = '',
    this.version = '',
    required this.fingerprint,
    this.mayApprove = true,
    required this.endpoint,
    DateTime? fetched,
  }) : fetched = fetched ?? DateTime.now();

  factory WaitingMove.fromJson(Map<String, Object?> json, {DateTime? fetched}) {
    String text(Map<String, Object?>? from, String key) => switch (from?[key]) {
      final String value => value,
      _ => '',
    };
    int number(String key) => switch (json[key]) {
      final int value => value,
      _ => 0,
    };
    final seen = json['seen'] is Map<String, Object?>
        ? json['seen'] as Map<String, Object?>
        : null;
    final says = json['says'] is Map<String, Object?>
        ? json['says'] as Map<String, Object?>
        : null;
    return WaitingMove(
      id: text(json, 'id'),
      state: text(json, 'state'),
      started: number('started'),
      expiresIn: number('expires_in'),
      ip: text(seen, 'ip'),
      network: text(seen, 'network'),
      place: text(seen, 'place'),
      name: text(says, 'name'),
      platform: text(says, 'platform'),
      version: text(says, 'version'),
      fingerprint: text(json, 'fingerprint'),
      mayApprove: json['may_approve'] == true,
      endpoint: text(json, 'endpoint').toLowerCase(),
      fetched: fetched,
    );
  }

  final String id;

  /// "waiting", or "two_at_once": another computer asked at the same time,
  /// so both ended.
  final String state;

  /// When it asked, in unix seconds on the service's clock.
  final int started;

  /// Seconds it still waited when fetched.
  final int expiresIn;

  /// What the service saw: the address, its network's owner and its place
  /// ("" when unknown).
  final String ip;
  final String network;
  final String place;

  /// What the computer calls itself, cleaned by the service.
  final String name;
  final String platform;
  final String version;

  /// `hex(sha256(receive key))[..16]`, sent back with the decision.
  final String fingerprint;

  /// Whether this device signed in by email itself, as an approver must.
  final bool mayApprove;

  /// The destination's receive key (64 hex), which the approval names.
  final String endpoint;

  /// When this device fetched it.
  final DateTime fetched;

  bool get twoAtOnce => state == 'two_at_once';

  /// The receive key's fingerprint, made here from [endpoint]: what the
  /// card shows and the approval carries.
  String? get code => endpointFingerprint(endpoint);

  /// Whether the service's [fingerprint] is that of the key it listed; a
  /// request where it isn't can't be approved.
  bool get keyMatches => code != null && code == fingerprint.toLowerCase();

  /// The request's end, on the service's clock.
  int get end => started + moveRequestLifetime;

  /// The service's clock at [now]: its end less what was left, plus the
  /// time since.
  int serviceNow(DateTime now) =>
      end - expiresIn + now.difference(fetched).inSeconds;

  /// Seconds left at [now].
  int left(DateTime now) => end - serviceNow(now);
}

/// This device's calls for moves to the account service.
abstract interface class MoveApprovals {
  /// Whether this device is signed in to the account, so it can look.
  bool get signedIn;

  /// What tells when [signedIn] changes (a sign-out while it shows).
  Listenable get changes;

  /// The device's account key (hex), which signs its approval.
  String get accountKey;

  /// The account's move waiting, if any, and one cancelled lately.
  Future<List<WaitingMove>> moves();

  /// The person's decision on [id] (`POST /v1/devices/authorizations/<id>`).
  Future<void> decide(String id, Map<String, Object?> decision);
}

/// [MoveApprovals] through this device's own sign-in to the account.
class DeviceMoveApprovals implements MoveApprovals {
  const DeviceMoveApprovals(this.account);

  final DeviceAccount account;

  @override
  bool get signedIn => account.signedIn;

  @override
  Listenable get changes => account;

  @override
  String get accountKey => account.key;

  @override
  Future<List<WaitingMove>> moves() async {
    final answer = await account.signed('GET', '/v1/devices/moves');
    final now = DateTime.now();
    return [
      for (final move in answer['moves'] as List? ?? const [])
        if (move is Map<String, Object?>)
          WaitingMove.fromJson(move, fetched: now),
    ];
  }

  @override
  Future<void> decide(String id, Map<String, Object?> decision) =>
      account.signed('POST', '/v1/devices/authorizations/$id', decision);
}

/// What this Claktory's word for the device says of itself: the account
/// and the Claktory (its payload, before the signature). Null when it
/// doesn't read.
({String accountId, String claktoryId})? allowedWordSays(String word) {
  final payload = word.split('.').first;
  try {
    final json = jsonDecode(
      utf8.decode(base64Url.decode(base64Url.normalize(payload))),
    );
    if (json is! Map<String, Object?>) return null;
    final account = json['account_id'];
    final claktory = json['claktory_id'];
    if (account is! String || claktory is! String) return null;
    if (account.isEmpty || claktory.isEmpty) return null;
    return (accountId: account, claktoryId: claktory);
  } on FormatException {
    return null;
  }
}

/// The waiting move's card under Settings → Move, while this device is
/// signed in to the account; nothing otherwise. Asks the service every few
/// seconds while it's on screen, never in the background.
class AccountMoveApproval extends StatefulWidget {
  const AccountMoveApproval({
    super.key,
    this.approvals,
    this.interval = const Duration(seconds: 5),
  });

  /// The service's calls; this device's own sign-in when null.
  final MoveApprovals? approvals;
  final Duration interval;

  @override
  State<AccountMoveApproval> createState() => _AccountMoveApprovalState();
}

class _AccountMoveApprovalState extends State<AccountMoveApproval> {
  MoveApprovals? _approvals;
  Backend? _backend;
  Timer? _timer;
  List<WaitingMove> _moves = const [];

  /// The request being answered, while it is.
  String? _busy;
  String? _error;

  /// Answered here, by id: what to say once it's no longer listed.
  final _answered = <String, String>{};

  MoveApprovals? _approvalsOf(BuildContext context) =>
      widget.approvals ??
      DeviceMoveApprovals(AppScope.scope(context).connections.account);

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final approvals = _approvalsOf(context);
    if (_approvals == null) {
      _approvals = approvals;
      // Signed out while it shows: it goes at once.
      approvals?.changes.addListener(_changed);
      // Connected again once this device signed in.
      _backend = AppScope.of(context)..cannotApprove.addListener(_changed);
      _timer = Timer.periodic(widget.interval, (_) => unawaited(_look()));
      unawaited(_look());
    }
  }

  @override
  void dispose() {
    _timer?.cancel();
    _approvals?.changes.removeListener(_changed);
    _backend?.cannotApprove.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  Future<void> _look() async {
    final approvals = _approvals;
    if (approvals == null || !approvals.signedIn || _busy != null) return;
    try {
      final moves = await approvals.moves();
      if (!mounted) return;
      setState(() {
        // Another request came: what was said of the last one goes.
        if (moves.any((move) => !_answered.containsKey(move.id))) {
          _answered.clear();
        }
        _moves = moves;
      });
    } on Object {
      // Whatever kept it from looking (the service, or this device's key
      // not there): said when the person acts; looking again in a moment.
    }
  }

  Future<void> _decide(WaitingMove move, {required bool approve}) async {
    final approvals = _approvals;
    if (approvals == null || _busy != null) return;
    final backend = AppScope.of(context);
    final account = AppScope.scope(context).connections.account;
    final name = claktoryNameInText(backend);
    setState(() {
      _busy = move.id;
      _error = null;
    });
    try {
      if (approve) {
        // Never a key other than the one whose code the card shows.
        final code = move.code;
        if (code == null || !move.keyMatches) throw const _Mismatch();
        // This Claktory vouches for this device, for this request only;
        // its word names the account and the Claktory that moves.
        final allowed = await backend.proveAllowedDevice(move.id);
        final says = allowedWordSays(allowed);
        if (says == null) {
          throw const RequestFailure(
            "Claktory's word for this device didn't read.",
          );
        }
        final now = DateTime.now();
        final iat = move.serviceNow(now);
        if (iat >= move.end) throw const _Ended();
        final approval = await deviceApprover.approveMove(
          // The approver reads this device's key itself.
          account: account,
          requestId: move.id,
          accountId: says.accountId,
          sourceClaktoryId: says.claktoryId,
          endpoint: move.endpoint,
          fingerprint: code,
          iat: iat,
          // Never later than the request's end (Fugo).
          exp: move.end,
          // The prompt names the Claktory and the code to compare.
          reason:
              'approve moving $name to the computer with code '
              '${fingerprintGroups(code)}',
        );
        await approvals.decide(move.id, {
          'approve': true,
          'fingerprint': code,
          'allowed': allowed,
          'move_approval': approval,
        });
        _answered[move.id] =
            'Approved. $name starts moving to that computer in a moment.';
      } else {
        await approvals.decide(move.id, {
          'approve': false,
          'fingerprint': move.fingerprint,
        });
        _answered[move.id] = "Declined. $name stays where it is.";
      }
      if (mounted) {
        setState(
          () => _moves = [
            for (final m in _moves)
              if (m.id != move.id) m,
          ],
        );
      }
    } on OwnerNotAuthenticated catch (refused) {
      _say(switch (refused.reason) {
        // The person changed their mind: nothing to say.
        OwnerAuthFailure.cancelled => null,
        OwnerAuthFailure.failed =>
          "This device couldn't confirm it's you, so nothing was approved.",
        OwnerAuthFailure.unavailable =>
          'To approve a move here, set up a passcode, Touch ID or a login '
              'password on this device first.',
      });
    } on UnimplementedError {
      _say("This device can't approve moves yet.");
    } on StateError {
      // Signed out meanwhile: no key to sign with.
      _say('Sign in to your Claktory account on this device to approve moves.');
    } on ArgumentError {
      // A request the signer won't sign as it is (its times or code).
      _say(
        "This request can't be approved as it is, so nothing was signed. "
        'Start the move again on the other computer.',
      );
    } on _Ended {
      _say(_endedWords);
    } on _Mismatch {
      _say(_mismatchWords);
    } on RequestFailure catch (failure) {
      _say(failure.message);
    } on AccountServiceError catch (error) {
      _say(switch (error.code) {
        'not_allowed_to_approve' =>
          "This device can't approve this move. Approve it on a device you "
              'signed in to your account with your email, connected to $name.',
        'fingerprint_changed' =>
          'The computer asking changed since this was shown, so nothing was '
              '${approve ? 'approved' : 'decided'}. Check it again.',
        'expired_token' => _endedWords,
        'not_found' => _endedWords,
        'unavailable' =>
          "Your account couldn't check your devices right now, so nothing "
              'was decided. Try again.',
        _ =>
          error.message.isEmpty ? "Couldn't send your answer." : error.message,
      });
    } on AccountServiceUnreachable {
      _say("Can't reach your account right now, so nothing was decided.");
    } finally {
      if (mounted) setState(() => _busy = null);
    }
  }

  static const _mismatchWords =
      "This request's code doesn't match the computer it names, so it can't "
      'be approved. Decline it and start the move again.';

  static const _endedWords =
      'This request ended. Start the move again on the other computer.';

  void _say(String? words) {
    if (mounted) setState(() => _error = words);
  }

  @override
  Widget build(BuildContext context) {
    final approvals = _approvals;
    if (approvals == null || !approvals.signedIn) {
      return const SizedBox.shrink();
    }
    final answered = [
      for (final MapEntry(:key, :value) in _answered.entries)
        if (!_moves.any((move) => move.id == key)) value,
    ];
    if (_moves.isEmpty && answered.isEmpty && _error == null) {
      return const SizedBox.shrink();
    }
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Padding(
      padding: const EdgeInsets.only(top: Space.m),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          for (final move in _moves)
            _MoveCard(
              key: ValueKey('move-request:${move.id}'),
              move: move,
              busy: _busy == move.id,
              onApprove: () => _decide(move, approve: true),
              onDecline: () => _decide(move, approve: false),
            ),
          for (final words in answered)
            Text(words, style: type.caption.copyWith(color: palette.textMuted)),
          if (_error case final error?) ...[
            const SizedBox(height: Space.xs),
            Text(error, style: type.caption.copyWith(color: palette.danger)),
          ],
        ],
      ),
    );
  }
}

class _Ended implements Exception {
  const _Ended();
}

class _Mismatch implements Exception {
  const _Mismatch();
}

/// One request: what the service saw first, then what the computer says of
/// itself, its key's fingerprint small, and the answers.
class _MoveCard extends StatelessWidget {
  const _MoveCard({
    super.key,
    required this.move,
    required this.busy,
    required this.onApprove,
    required this.onDecline,
  });

  final WaitingMove move;
  final bool busy;
  final VoidCallback onApprove;
  final VoidCallback onDecline;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final backend = AppScope.of(context);
    final name = claktoryNameInText(backend);
    final muted = type.caption.copyWith(color: palette.textMuted);
    // Why this device can't approve it, said in place of Approve: the
    // service's word first, then this Claktory's.
    final cannot = backend.cannotApprove.value;
    final whyNot = !move.mayApprove
        ? "This device can't approve moves: it signed in to your account "
              'through a Claktory. Approve on a device you signed in to with '
              'your email.'
        : cannot.isEmpty
        ? null
        : cannot;
    final now = DateTime.now();
    final left = move.left(now);
    final asked = moveRequestLifetime - left;
    String shown(String text) => text.isEmpty
        ? 'Unknown'
        : visibleCodes(text.replaceAll(RegExp(r'\s+'), ' '));
    final says = [
      for (final part in [move.name, move.platform, move.version])
        if (part.isNotEmpty) shown(part),
    ].join(' · ');
    Widget fact(String label, String value) => Padding(
      padding: const EdgeInsets.only(top: Space.xs),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(width: 120, child: Text(label, style: muted)),
          Expanded(
            child: Text(value, style: type.body.copyWith(color: palette.text)),
          ),
        ],
      ),
    );
    return Container(
      padding: const EdgeInsets.all(Space.m),
      decoration: BoxDecoration(
        border: Border.all(color: palette.hairline),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            move.twoAtOnce
                ? 'Two computers asked to receive $name at once'
                : 'A computer asked to receive $name',
            style: type.body.copyWith(
              color: palette.text,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(height: Space.xs),
          Text(
            move.twoAtOnce
                ? 'So neither can. Start the move again on the computer you '
                      'want, and only there.'
                : 'Approving moves everything in $name there: agents, '
                      'conversations, folders, secrets and sign-ins. Approve '
                      'only if you started this move on that computer.',
            style: muted,
          ),
          if (!move.twoAtOnce) ...[
            const SizedBox(height: Space.s),
            fact('Address', shown(move.ip)),
            fact('Network', shown(move.network)),
            fact('Place', shown(move.place)),
            if (says.isNotEmpty) fact('It calls itself', says),
            fact(
              'Asked',
              '${asked < 60 ? 'just now' : '${asked ~/ 60} min ago'}'
                  '${left > 0 ? ' · ends in ${(left + 59) ~/ 60} min' : ' · ended'}',
            ),
            const SizedBox(height: Space.s),
            // Made here from the key the approval names, to compare with
            // what `receive --email` printed on that computer.
            if (move.keyMatches)
              Text(
                "This computer's code: ${fingerprintGroups(move.code!)}. It "
                'must match the code on the computer you\'re moving to.',
                key: const ValueKey('move-code'),
                style: type.body.copyWith(color: palette.text),
              )
            else
              Text(
                _AccountMoveApprovalState._mismatchWords,
                style: type.body.copyWith(color: palette.danger),
              ),
            const SizedBox(height: Space.m),
            if (whyNot != null) ...[
              Text(whyNot, key: const ValueKey('move-why-not'), style: muted),
              const SizedBox(height: Space.s),
            ],
            Wrap(
              spacing: Space.s,
              runSpacing: Space.s,
              children: [
                if (whyNot == null && move.keyMatches)
                  Button(
                    label: busy ? 'Approving…' : 'Approve move…',
                    onPressed: busy || left <= 0 ? null : onApprove,
                  ),
                Button(
                  label: 'Decline',
                  onPressed: busy || left <= 0 ? null : onDecline,
                ),
              ],
            ),
          ],
        ],
      ),
    );
  }
}
