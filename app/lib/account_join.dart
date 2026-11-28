// A computer asking to join the person's Claktory account (the account
// service's API.md, "Adding a computer with the person's approval"). An
// agent set Claktory up on a server with `factoryd account authorize
// --email`; the account service holds its request for 10 minutes, and the
// Claktory this device shows only rings the bell
// (ClaktoryAccount.authorizations_waiting): it never carries the request or
// the answer. Needs you lists one quiet item per id (it
// counts, with no push and no sound: anyone with the email can start one),
// and its card fetches the request itself, signed by this device's account
// key (GET /v1/devices/authorizations).
//
// The card leads with what the service saw (network, address, place, when),
// then what the computer calls itself, labelled as its own words, what
// approving gives, "Connect this <device> to it" (on), and the backend key's
// fingerprint, small, for support. Approve only from the person's tap or key
// in the app, never a notification, a URL or launch arguments: the device's
// owner authenticates first (device_approval.dart's confirmOwner: Touch ID
// or the login password, Face ID or the passcode), then this Claktory
// vouches for this device (ProveAllowedDevice, good for 120 s), and the
// decision goes signed with the fingerprint the card showed.
import 'dart:async';

import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'card_answer.dart' show CardAnswer, PublishesAnswer;
import 'connections.dart' show Connections, RemoteFactory;
import 'controls.dart';
import 'decision.dart' show CardRow;
import 'device_account.dart'
    show
        AccountClaktory,
        AccountServiceError,
        AccountServiceUnreachable,
        DeviceAccount;
import 'device_approval.dart';
import 'devices.dart' show thisDeviceKind;
import 'identity.dart';
import 'message_time.dart' show clockTime;
import 'mobile/metrics.dart' show TypeScale;
import 'move_approval.dart' show fingerprintGroups;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'settings_view.dart' show thisDeviceTitle;
import 'visible_codes.dart' show nameInText, visibleCodes;

/// How long a request waits for the person, from its start (API.md: "for 10
/// minutes").
const joinRequestLifetime = 600;

/// A computer's request to join, as this device last fetched it.
class JoinRequest {
  JoinRequest({
    required this.id,
    this.state = 'waiting',
    required this.expiresIn,
    this.ip = '',
    this.network = '',
    this.place = '',
    this.name = '',
    this.platform = '',
    this.version = '',
    this.fingerprint = '',
    this.mayApprove = false,
    this.answeredBy = '',
    this.answeredAt = 0,
    DateTime? fetched,
  }) : fetched = fetched ?? DateTime.now();

  factory JoinRequest.fromJson(Map<String, Object?> json, {DateTime? fetched}) {
    String text(Object? from, String key) => switch (from) {
      final Map<String, Object?> map when map[key] is String =>
        map[key]! as String,
      _ => '',
    };
    return JoinRequest(
      id: text(json, 'id'),
      state: text(json, 'state'),
      expiresIn: switch (json['expires_in']) {
        final int value => value,
        _ => 0,
      },
      ip: text(json['seen'], 'ip'),
      network: text(json['seen'], 'network'),
      place: text(json['seen'], 'place'),
      name: text(json['says'], 'name'),
      platform: text(json['says'], 'platform'),
      version: text(json['says'], 'version'),
      fingerprint: text(json, 'fingerprint'),
      mayApprove: json['may_approve'] == true,
      answeredBy: text(json, 'answered_by'),
      answeredAt: switch (json['answered_at']) {
        final int value => value,
        _ => 0,
      },
      fetched: fetched,
    );
  }

  final String id;

  /// "waiting"; "two_at_once": another computer asked at the same time,
  /// so both ended; or "approved" or "declined", for 10 minutes after one
  /// of the account's devices answered it.
  final String state;

  /// Seconds it still waited when fetched.
  final int expiresIn;

  /// What the service saw: the address, its network's owner and its place
  /// ("" when unknown).
  final String ip;
  final String network;
  final String place;

  /// What the computer calls itself, cleaned by the service (printable, at
  /// most 40 characters): its own words, unchecked.
  final String name;
  final String platform;
  final String version;

  /// `hex(sha256(backend key))[..16]`, shown small and sent back with the
  /// decision: a different computer is refused.
  final String fingerprint;

  /// Whether this device signed in to the account by email itself, as an
  /// approver must (the Claktory's word is checked when it approves).
  final bool mayApprove;

  /// The device that answered it, cleaned by the service; "" when that
  /// device is no longer on the account, or it isn't answered.
  final String answeredBy;

  /// When it was answered, in unix seconds on the service's clock; 0 when
  /// it isn't.
  final int answeredAt;

  /// When this device fetched it.
  final DateTime fetched;

  bool get twoAtOnce => state == 'two_at_once';

  /// Still waiting for an answer.
  bool get waiting => state == 'waiting';

  /// How one of the account's devices answered it: "Approved on Matt's
  /// iPhone"; null while it isn't answered.
  String? get answered {
    final on = answeredBy.trim().isEmpty
        ? 'another device'
        : visibleCodes(answeredBy.replaceAll(RegExp(r'\s+'), ' '));
    return switch (state) {
      'approved' => 'Approved on $on',
      'declined' => 'Declined on $on',
      _ => null,
    };
  }

  /// When it asked, on this device's clock: what was used of its 10
  /// minutes before it was fetched.
  DateTime get asked =>
      fetched.subtract(Duration(seconds: joinRequestLifetime - expiresIn));

  /// Seconds left at [now].
  int left(DateTime now) => expiresIn - now.difference(fetched).inSeconds;
}

/// How long [request] still waits, for its row and card: "9 min left",
/// "Less than a minute left"; empty once it's over.
String joinTimeLeft(JoinRequest request, DateTime now) {
  final left = request.left(now);
  if (left <= 0) return '';
  if (left < 60) return 'Less than a minute left';
  return '${left ~/ 60} min left';
}

/// What a computer's platform says, as people say it: "Linux" for
/// linux-x86_64; another as it came.
String joinPlatform(String platform) {
  final lower = platform.toLowerCase();
  if (lower.startsWith('linux')) return 'Linux';
  if (lower.startsWith('macos') || lower.startsWith('darwin')) return 'macOS';
  if (lower.startsWith('windows')) return 'Windows';
  return platform;
}

/// The name a computer gave, for a sentence: isolated so it can't turn the
/// sentence around; "a new Claktory" when it gave none.
String joinNameInText(String name) =>
    name.trim().isEmpty ? 'a new Claktory' : nameInText(name);

/// The owner-authentication prompt's words for approving [request]: the
/// address the service saw it come from, never what it calls itself (Fugo
/// 51502). In lower case: device_approval.dart's prompt capitalises it
/// where the system doesn't put "Claktory is trying to" first (Trish
/// 51606).
String joinOwnerReason(JoinRequest request) => request.ip.isEmpty
    ? 'approve a new Claktory joining your account'
    : 'approve the computer at ${visibleCodes(request.ip)} joining your '
          'account';

/// What a join approved here says when the new Claktory isn't on the
/// account's list in time to connect to it.
const joinConnectLater =
    "When it's ready, connect to it in Add a Claktory, under On your "
    'account.';

/// This device's calls about joins to the account service.
abstract interface class JoinApprovals {
  /// Whether this device is signed in to the account, so it can look.
  bool get signedIn;

  /// What tells when [signedIn] or [email] changes.
  Listenable get changes;

  /// The email it's signed in with.
  String get email;

  /// The account's request waiting, if any, and one cancelled lately.
  Future<List<JoinRequest>> requests();

  /// The person's decision on [id] (`POST /v1/devices/authorizations/<id>`).
  Future<void> decide(String id, Map<String, Object?> decision);

  /// The account's Claktories, to find the one that joined.
  Future<List<AccountClaktory>> claktories();
}

/// [JoinApprovals] through this device's own sign-in to the account.
class DeviceJoinApprovals implements JoinApprovals {
  const DeviceJoinApprovals(this.account);

  final DeviceAccount account;

  @override
  bool get signedIn => account.signedIn;

  @override
  Listenable get changes => account;

  @override
  String get email => account.email;

  @override
  Future<List<JoinRequest>> requests() async {
    final answer = await account.signed('GET', '/v1/devices/authorizations');
    final now = DateTime.now();
    return [
      for (final request in answer['authorizations'] as List? ?? const [])
        if (request is Map<String, Object?> && request['kind'] == 'claktory')
          JoinRequest.fromJson(request, fetched: now),
    ];
  }

  @override
  Future<void> decide(String id, Map<String, Object?> decision) =>
      account.signed('POST', '/v1/devices/authorizations/$id', decision);

  @override
  Future<List<AccountClaktory>> claktories() => account.claktories();
}

/// This device's own calls about joins.
JoinApprovals joinApprovalsOf(BuildContext context) =>
    DeviceJoinApprovals(AppScope.scope(context).connections.account);

/// [backend]'s joins, watched through this device's sign-in: what a list
/// of Needs you calls as it builds, so a request that rang is fetched for
/// its row's time left.
JoinWatch watchJoins(BuildContext context, Backend backend) =>
    JoinWatch.of(backend)..watchWith(joinApprovalsOf(context));

/// The requests answered or set aside on this device while the app runs:
/// Needs you lists them no longer, though the bell may still ring for a
/// minute until the Claktory hears.
final _settled = <String>{};

/// Forgets what was answered here, for tests.
@visibleForTesting
void forgetSettledJoins() => _settled.clear();

/// Whether request [id] was answered or set aside here.
bool joinSettled(String id) => _settled.contains(id);

/// The ids of the computers asking to join [workspace]'s account, as its
/// Claktory last heard, less those answered here.
List<String> joinsWaiting(Workspace workspace) => [
  for (final id in workspace.account.authorizationsWaiting)
    if (id.isNotEmpty && !_settled.contains(id)) id,
];

/// What this device knows of the joins of one Claktory's account while the
/// app runs: each request as last fetched, and how those answered here
/// ended. Fetches when the bell rings for a request it hasn't seen, and
/// ticks while one waits, so its time left counts down.
class JoinWatch extends ChangeNotifier {
  JoinWatch._(this._backend) {
    _backend.workspace.addListener(_rang);
  }

  static final _watches = Expando<JoinWatch>('JoinWatch');

  /// [backend]'s, one per Claktory.
  static JoinWatch of(Backend backend) =>
      _watches[backend] ??= JoinWatch._(backend);

  final Backend _backend;
  JoinApprovals? _approvals;
  Timer? _tick;
  var _looking = false;

  final _requests = <String, JoinRequest>{};
  final _outcomes = <String, String>{};

  /// [id] as last fetched; null before it was.
  JoinRequest? request(String id) => _requests[id];

  /// How [id] ended, answered here; null otherwise.
  String? outcome(String id) => _outcomes[id];

  /// What it looks through, once set.
  JoinApprovals? get approvals => _approvals;

  /// Looks through [approvals] (this device's sign-in, or a test's), the
  /// first set, fetching at once when a request rang that it hasn't seen.
  void watchWith(JoinApprovals approvals) {
    if (_approvals != null) return;
    _approvals = approvals;
    _rang();
  }

  /// The ids the bell rang for when it last changed.
  var _rung = const <String>{};

  void _rang() {
    final waiting = joinsWaiting(_backend.workspace.workspace);
    final rung = waiting.toSet();
    // One it hasn't seen, or one that stopped ringing, not answered here:
    // the service says what it is, or how it was answered.
    if (waiting.any((id) => !_requests.containsKey(id)) ||
        _rung.difference(rung).any((id) => _outcomes[id] == null)) {
      unawaited(look());
    }
    _rung = rung;
    _ticking();
  }

  /// Fetches the account's requests again.
  Future<void> look() async {
    final approvals = _approvals;
    if (approvals == null || !approvals.signedIn || _looking) return;
    _looking = true;
    try {
      final requests = await approvals.requests();
      for (final request in requests) {
        _requests[request.id] = request;
      }
      _ticking();
      notifyListeners();
    } on Object {
      // Whatever kept it from looking (the service, or this device's key
      // not there): said when the person acts; fetched again when the card
      // opens.
    } finally {
      _looking = false;
    }
  }

  /// Every 10 seconds while one rang that still has time and something
  /// shows it, so "9 min left" counts down where it is.
  void _ticking() {
    final now = DateTime.now();
    final counting =
        hasListeners &&
        joinsWaiting(_backend.workspace.workspace)
            .any((id) => (_requests[id]?.left(now) ?? 0) > 0);
    if (counting && _tick == null) {
      _tick = Timer.periodic(const Duration(seconds: 10), (_) {
        notifyListeners();
        _ticking();
      });
    } else if (!counting) {
      _tick?.cancel();
      _tick = null;
    }
  }

  /// [id] was answered here, or set aside: Needs you lists it no longer,
  /// and an open card says [outcome].
  void settle(String id, String outcome) {
    _settled.add(id);
    _outcomes[id] = outcome;
    _ticking();
    notifyListeners();
  }

  @override
  void addListener(VoidCallback listener) {
    super.addListener(listener);
    _ticking();
  }

  @override
  void removeListener(VoidCallback listener) {
    super.removeListener(listener);
    _ticking();
  }

  @override
  void dispose() {
    _tick?.cancel();
    _backend.workspace.removeListener(_rang);
    super.dispose();
  }
}

/// The row's words for request [id]: who asks, and its time left (or how
/// it ended).
({String who, String what}) joinWords(Backend backend, String id) {
  final watch = JoinWatch.of(backend);
  final request = watch.request(id);
  final what = switch (watch.outcome(id)) {
    final outcome? => outcome,
    _ when request == null => 'Waiting for your answer',
    _ when request.answered != null => request.answered!,
    _ when request.twoAtOnce => _twoAtOnceWords,
    _ => switch (joinTimeLeft(request, DateTime.now())) {
      '' => _expiredWords,
      final left => left,
    },
  };
  return (who: 'A new Claktory asks to join your account', what: what);
}

/// The line under the title once the card is open: only what the card
/// doesn't say itself, its time left while it waits.
String joinOpenLine(Backend backend, String id) {
  final watch = JoinWatch.of(backend);
  final request = watch.request(id);
  if (request == null ||
      !request.waiting ||
      watch.outcome(id) != null ||
      !joinsWaiting(backend.workspace.workspace).contains(id)) {
    return '';
  }
  return joinTimeLeft(request, DateTime.now());
}

const _twoAtOnceWords =
    'Two computers asked at once, so neither joined. Start again on the one '
    'you want.';
const _expiredWords = 'Expired before anyone approved it';

/// Finds the Claktory that just joined among [approvals]' list, every
/// [every] for up to [giveUp]: the one not in [before] with the
/// [fingerprint] the person approved (so never another that joined at
/// once) and an address to connect to. Null if none came.
Future<AccountClaktory?> findJoined(
  JoinApprovals approvals,
  Set<String> before,
  String fingerprint, {
  Duration every = const Duration(seconds: 5),
  Duration giveUp = const Duration(minutes: 3),
}) async {
  // Nothing approved to match.
  if (fingerprint.isEmpty) return null;
  final tries = giveUp.inMilliseconds ~/ every.inMilliseconds;
  for (var tried = 0; ; tried++) {
    try {
      final listed = await approvals.claktories();
      final joined = listed
          .where(
            (c) =>
                !before.contains(c.id) &&
                c.fingerprint == fingerprint.toLowerCase() &&
                c.endpointId.isNotEmpty,
          )
          .firstOrNull;
      if (joined != null) return joined;
    } on AccountServiceError {
      // Looking again in a moment.
    } on AccountServiceUnreachable {
      // The same.
    }
    if (tried >= tries) return null;
    await Future<void>.delayed(every);
  }
}

/// Request [id]'s card in Needs you: what the service saw, what the
/// computer calls itself, what approving gives, and Decline · Approve.
class AccountJoinCard extends StatefulWidget {
  const AccountJoinCard({
    super.key,
    required this.id,
    this.approvals,
    this.interval = const Duration(seconds: 5),
    this.findEvery = const Duration(seconds: 5),
  });

  final String id;

  /// The service's calls; this device's own sign-in when null.
  final JoinApprovals? approvals;

  /// How often it asks the service again while it's open.
  final Duration interval;

  /// How often it looks for the Claktory that joined, to connect to it.
  final Duration findEvery;

  @override
  State<AccountJoinCard> createState() => _AccountJoinCardState();
}

class _AccountJoinCardState extends State<AccountJoinCard>
    with PublishesAnswer {
  late final Backend _backend = AppScope.of(context);
  late final JoinWatch _watch = JoinWatch.of(_backend);
  late final JoinApprovals _approvals =
      widget.approvals ?? _watch.approvals ?? joinApprovalsOf(context);
  Timer? _timer;
  var _busy = false;
  var _connect = true;
  String? _error;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_timer == null) {
      _watch.addListener(_changed);
      _backend.workspace.addListener(_changed);
      // Connected again once this device signed in.
      _backend.cannotApprove.addListener(_changed);
      // Signed out, or into another account, while it shows.
      _approvals.changes.addListener(_changed);
      _watch.watchWith(_approvals);
      unawaited(_watch.look());
      _timer = Timer.periodic(widget.interval, (_) {
        if (!_busy && _unsettled) unawaited(_watch.look());
      });
    }
  }

  @override
  void dispose() {
    _timer?.cancel();
    _watch.removeListener(_changed);
    _backend.workspace.removeListener(_changed);
    _backend.cannotApprove.removeListener(_changed);
    _approvals.changes.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  /// Whether the bell still rings for it and it wasn't answered here.
  bool get _waiting =>
      joinsWaiting(_backend.workspace.workspace).contains(widget.id);

  JoinRequest? get _request => _watch.request(widget.id);

  /// Whether there's more for the service to say: it rings, or it stopped
  /// ringing and how it was answered isn't known yet.
  bool get _unsettled =>
      _watch.outcome(widget.id) == null &&
      (_waiting || (_request?.waiting ?? true));

  /// Whether Approve can be pressed now.
  bool get _canApprove {
    final request = _request;
    return request != null &&
        _waiting &&
        request.waiting &&
        request.mayApprove &&
        _backend.cannotApprove.value.isEmpty &&
        request.left(DateTime.now()) > 0;
  }

  /// Why this device can't approve it, said in place of Approve; null
  /// when it can. The service's word first, then this Claktory's (its
  /// Welcome's cannot_approve: a device paired with a code before it
  /// showed the account, say).
  String? _whyNot(JoinRequest request) {
    if (!request.mayApprove) {
      return "This device can't approve it: it signed in to your account "
          'through a Claktory. Approve it on a device you signed in to with '
          'your email.';
    }
    final cannot = _backend.cannotApprove.value;
    return cannot.isEmpty ? null : cannot;
  }

  /// How it ended, once it did: answered here, ran out, or answered on
  /// another device. Null while it waits.
  String? get _ended {
    if (_watch.outcome(widget.id) case final outcome?) return outcome;
    final request = _request;
    // Answered on another device, which the service names.
    if (request?.answered case final answered?) return answered;
    if (request != null && request.twoAtOnce) return _twoAtOnceWords;
    if (request != null && request.left(DateTime.now()) <= 0) {
      return _expiredWords;
    }
    if (!_waiting) return 'Answered on another device';
    return null;
  }

  Future<void> _approve() async {
    final request = _request;
    if (request == null || !_canApprove || _busy) return;
    final name = joinNameInText(request.name);
    final connect = _connect;
    // Taken now: it may connect after the card has gone.
    final connections = AppScope.scope(context).connections;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      // The person, deliberately, on this device: nothing goes before.
      // In what the service saw, never the computer's own name: the
      // system's dialog reads with the system's authority.
      await deviceApprover.confirmOwner(reason: joinOwnerReason(request));
      if (request.left(DateTime.now()) <= 0) {
        _watch.settle(request.id, _expiredWords);
        return;
      }
      // This Claktory vouches for this device, for this request only.
      final allowed = await _backend.proveAllowedDevice(request.id);
      // What's on the account already, to know the one that joins.
      Set<String>? before;
      if (connect) {
        try {
          before = {for (final c in await _approvals.claktories()) c.id};
        } on AccountServiceError {
          before = null;
        } on AccountServiceUnreachable {
          before = null;
        }
      }
      await _approvals.decide(request.id, {
        'approve': true,
        'fingerprint': request.fingerprint,
        'allowed': allowed,
      });
      final joined = request.name.trim().isEmpty
          ? 'A new Claktory joined your account'
          : '$name joined your account';
      _watch.settle(request.id, joined);
      if (before != null) {
        unawaited(
          _connectTo(
            connections,
            request.id,
            joined,
            before,
            request.fingerprint,
          ),
        );
      } else if (connect) {
        _watch.settle(request.id, '$joined. $joinConnectLater');
      }
    } on OwnerNotAuthenticated catch (refused) {
      // Nothing changes: the card stays as it was.
      _say(switch (refused.reason) {
        OwnerAuthFailure.cancelled => null,
        OwnerAuthFailure.failed =>
          "This device couldn't confirm it's you, so nothing was approved.",
        OwnerAuthFailure.unavailable =>
          'To approve here, set up a passcode, Touch ID or a login password '
              'on this device first.',
      });
    } on UnimplementedError {
      _say("This device can't approve new Claktories yet.");
    } on RequestFailure catch (failure) {
      _say(failure.message);
    } on AccountServiceError catch (error) {
      _refused(error, approving: true);
    } on AccountServiceUnreachable {
      _say(
        "Can't reach your account right now, so nothing was decided. Try "
        'again.',
      );
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// Connects to the Claktory that joined once the account lists it: its
  /// tile in the rail (on a phone, the Claktories list) connects, and the
  /// window stays here. Runs on if the card goes.
  Future<void> _connectTo(
    Connections connections,
    String id,
    String joined,
    Set<String> before,
    String fingerprint,
  ) async {
    final claktory = await findJoined(
      _approvals,
      before,
      fingerprint,
      every: widget.findEvery,
    );
    if (claktory == null) {
      _watch.settle(id, '$joined. $joinConnectLater');
      return;
    }
    connections.add(RemoteFactory.fromAccount(claktory));
  }

  Future<void> _decline() async {
    final request = _request;
    if (request == null || _busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await _approvals.decide(request.id, {
        'approve': false,
        'fingerprint': request.fingerprint,
      });
      _watch.settle(request.id, 'Declined. Nothing joined your account.');
    } on AccountServiceError catch (error) {
      _refused(error, approving: false);
    } on AccountServiceUnreachable {
      _say(
        "Can't reach your account right now, so nothing was decided. Try "
        'again.',
      );
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// Sets it aside here once it ended by itself.
  void _ok() {
    _watch.settle(widget.id, _ended ?? _expiredWords);
  }

  void _refused(AccountServiceError error, {required bool approving}) {
    switch (error.code) {
      case 'expired_token' || 'not_found':
        _watch.settle(widget.id, _expiredWords);
      default:
        _say(switch (error.code) {
          'not_allowed_to_approve' =>
            "This device can't approve it. Approve it on a device you signed "
                "in to with your email that's connected to one of your "
                'Claktories.',
          'fingerprint_changed' =>
            'The computer asking changed after this was shown, so nothing '
                'was ${approving ? 'approved' : 'decided'}. Check the details '
                'again.',
          'unavailable' =>
            "Couldn't check your devices just now, so nothing was decided. "
                'Try again.',
          _ =>
            error.message.isEmpty
                ? "Couldn't send your answer."
                : error.message,
        });
    }
  }

  void _say(String? words) {
    if (mounted) setState(() => _error = words);
  }

  @override
  CardAnswer answerNow() {
    final key = 'join:${widget.id}';
    if (_ended != null) {
      return CardAnswer(
        key: key,
        docked: false,
        yes: _watch.outcome(widget.id) == null ? 'OK' : '',
        no: '',
        onYes: _watch.outcome(widget.id) == null ? () async => _ok() : null,
      );
    }
    return CardAnswer(
      key: key,
      docked: false,
      yes: _canApprove ? 'Approve' : '',
      no: 'Decline',
      busy: _busy,
      error: _error,
      onYes: _canApprove && !_busy ? _approve : null,
      onNo: _request == null || _busy ? null : (_) => _decline(),
    );
  }

  @override
  Widget build(BuildContext context) {
    publishAnswer();
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final muted = type.caption.copyWith(color: palette.textMuted);
    final account = _backend.workspace.workspace.account;
    if (!_approvals.signedIn) {
      return Text(
        'To answer it, sign in on this device, in Settings → '
        '$thisDeviceTitle.',
        style: muted,
      );
    }
    if (account.email.isNotEmpty &&
        _approvals.email.toLowerCase() != account.email.toLowerCase()) {
      return Text(
        'This device is signed in to ${nameInText(_approvals.email)}, not '
        '${nameInText(account.email)}. Answer it on a device signed in to '
        '${nameInText(account.email)}.',
        style: muted,
      );
    }
    final request = _request;
    final ended = _ended;
    if (ended != null) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            ended,
            key: const ValueKey('join-outcome'),
            style: type.body.copyWith(color: palette.text),
          ),
          if (_watch.outcome(widget.id) == null) ...[
            const SizedBox(height: Space.m),
            CardRow(
              primary: Button(label: 'OK', primary: true, onPressed: _ok),
            ),
          ],
        ],
      );
    }
    if (request == null) {
      return Text(
        _error ?? 'Getting the request from your account…',
        style: _error == null
            ? muted
            : type.caption.copyWith(color: palette.danger),
      );
    }
    String shown(String text) => text.isEmpty
        ? 'Unknown'
        : visibleCodes(text.replaceAll(RegExp(r'\s+'), ' '));
    final says = [
      if (request.name.isNotEmpty) shown(request.name),
      if (request.platform.isNotEmpty) shown(joinPlatform(request.platform)),
    ].join(' · ');
    final now = DateTime.now();
    Widget fact(String label, String value, {String? key}) => Padding(
      padding: const EdgeInsets.only(top: Space.xs),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(width: 120, child: Text(label, style: muted)),
          Expanded(
            child: Text(
              value,
              key: key == null ? null : ValueKey(key),
              style: type.body.copyWith(color: palette.text),
            ),
          ),
        ],
      ),
    );
    final connectLabel = 'Connect this $thisDeviceKind to it';
    final onConnect = _busy ? null : (bool on) => setState(() => _connect = on);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          'Approve a new Claktory?',
          style: type.body.copyWith(
            color: palette.text,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: Space.xs),
        Text(
          'Approve only if you or one of your agents just set up Claktory on '
          'a computer. Once approved, it joins your account: it sees your '
          'other Claktories, and your devices trust it.',
          style: muted,
        ),
        const SizedBox(height: Space.s),
        // What the service saw first (Fugo's item 3).
        fact(
          'Network',
          [
            if (request.network.isNotEmpty) shown(request.network),
            shown(request.ip),
          ].join(' · '),
        ),
        fact('Place', shown(request.place)),
        fact(
          'Asked',
          '${clockTime(request.asked.millisecondsSinceEpoch, h24: MediaQuery.maybeAlwaysUse24HourFormatOf(context) ?? true)}'
              ' · ${joinTimeLeft(request, now)}',
          key: 'join-asked',
        ),
        // Then its own words, set apart as its own.
        if (says.isNotEmpty) fact('It calls itself', says),
        const SizedBox(height: Space.m),
        if (_whyNot(request) case final whyNot?)
          Text(whyNot, key: const ValueKey('join-why-not'), style: muted)
        else
          ToggleTap(
            value: _connect,
            onChanged: onConnect,
            // The whole row toggles it; on a touch screen the switch is a
            // 44-point target, so the row is too.
            child: Row(
              children: [
                Toggle(
                  value: _connect,
                  label: connectLabel,
                  onChanged: onConnect,
                ),
                const SizedBox(width: Space.m),
                Expanded(
                  child: Text(
                    connectLabel,
                    key: const ValueKey('join-connect'),
                    style: type.body.copyWith(color: palette.text),
                  ),
                ),
              ],
            ),
          ),
        if (request.fingerprint.isNotEmpty) ...[
          const SizedBox(height: Space.s),
          Text(
            'For support: ${fingerprintGroups(request.fingerprint)}',
            style: type.caption.copyWith(color: palette.textFaint),
          ),
        ],
        const SizedBox(height: Space.m),
        // The card's action row, as agents' requests have it: the error
        // at the left, Decline, then Approve filled at the end (Lucy's
        // needs-you-cards rule 2); on a phone the thumb row.
        CardRow(
          line: switch (_error) {
            final error? => Text(
              error,
              style: type.caption.copyWith(color: palette.danger),
            ),
            null => null,
          },
          quiet: [
            Button(
              label: 'Decline',
              onPressed: _busy || !_waiting ? null : _decline,
            ),
          ],
          primary: _whyNot(request) == null
              ? Button(
                  label: _busy ? 'Approving…' : 'Approve',
                  primary: true,
                  onPressed: _canApprove && !_busy ? _approve : null,
                )
              : null,
        ),
      ],
    );
  }
}
