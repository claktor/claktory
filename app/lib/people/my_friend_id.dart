import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../avatar.dart';
import '../controls.dart';
import '../devices.dart' show QrCode;
import '../forms.dart';
import '../identity.dart';
import '../mobile/metrics.dart';
import '../user_apps/session.dart';
import 'my_identity.dart';
import 'problems.dart';

// D3a: the person's own identity in People, the first of it they see:
// their name and avatar there, their secret friend ID (shown only when
// asked, copied, as a QR code, or reset), and whether anyone may send
// them a friend request. Straight from the account service as this
// device, so it works with every Claktory off. No requests or chats yet.

/// Opens My friend ID.
Future<void> showMyFriendId(BuildContext context) =>
    showModal(context, (context, close) => MyFriendIdView(close: close));

/// Stands in for the account service in tests.
MyIdentityApi Function(BuildContext context)? myIdentityApi;

class MyFriendIdView extends StatefulWidget {
  const MyFriendIdView({super.key, required this.close});

  final VoidCallback close;

  @override
  State<MyFriendIdView> createState() => _MyFriendIdViewState();
}

class _MyFriendIdViewState extends State<MyFriendIdView> {
  late final MyIdentityApi _api =
      myIdentityApi?.call(context) ??
      MyIdentityApi(UserAppsSession.of(context));

  MyIdentity? _me;

  /// The friend ID, while it's shown: only ever here, gone once hidden or
  /// closed.
  FriendId? _code;
  var _qr = false;
  var _busy = false;
  String? _error;

  /// The name being edited, or null.
  TextEditingController? _name;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  @override
  void dispose() {
    _code = null;
    _name?.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    try {
      final me = await _api.me();
      if (mounted) setState(() => _me = me);
    } on UserAppError catch (error) {
      if (mounted) setState(() => _error = problemText(error));
    }
  }

  /// Runs [request]; a change another device made first reads it again
  /// and says so.
  Future<void> _run(Future<void> Function() request) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await request();
    } on UserAppError catch (error) {
      if (error.problem == UserAppProblem.stale) {
        _code = null;
        _qr = false;
        try {
          final me = await _api.me();
          if (mounted) setState(() => _me = me);
        } on UserAppError {
          // Its error is the one to show.
        }
      }
      if (mounted) setState(() => _error = problemText(error));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  void _show() => _run(() async {
    final code = await _api.reveal();
    if (mounted) setState(() => _code = code);
  });

  void _hide() => setState(() {
    _code = null;
    _qr = false;
  });

  Future<void> _reset() async {
    final me = _me;
    if (me == null) return;
    var sure = false;
    await showModal(
      context,
      (context, close) => FormFrame(
        title: 'Reset your friend ID?',
        body:
            'Your friend ID stops working for good and you get a new one. '
            'Friends you already have stay your friends.',
        fields: const [],
        error: null,
        submitLabel: 'Reset',
        onSubmit: () {
          sure = true;
          close();
        },
        onCancel: close,
      ),
    );
    if (!sure || !mounted) return;
    await _run(() async {
      final code = await _api.reset(_code?.generation ?? me.generation);
      final fresh = await _api.me();
      if (mounted) {
        setState(() {
          _code = code;
          _me = fresh;
        });
      }
    });
  }

  void _editName() =>
      setState(() => _name = TextEditingController(text: _me?.name ?? ''));

  void _saveName() {
    final me = _me;
    final name = _name?.text.trim() ?? '';
    if (me == null || name.isEmpty) return;
    _run(() async {
      final changed = await _api.setProfile(me, name: name);
      if (mounted) {
        setState(() {
          _me = changed;
          _name?.dispose();
          _name = null;
        });
      }
    });
  }

  void _newAvatar() {
    final me = _me;
    if (me == null) return;
    _run(() async {
      final changed = await _api.setProfile(me, newAvatar: true);
      if (mounted) setState(() => _me = changed);
    });
  }

  void _requests(bool on) {
    final me = _me;
    if (me == null) return;
    _run(() async {
      final changed = await _api.setRequests(me, on);
      if (mounted) setState(() => _me = changed);
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final me = _me;
    return FormFrame(
      title: 'My friend ID',
      body: me == null
          ? null
          : 'People you give your friend ID to can send you a friend '
                'request. Nobody can find you any other way.',
      fields: [
        if (me == null && _error == null)
          Text(
            'Getting your friend ID…',
            style: type.body.copyWith(color: palette.textMuted),
          ),
        if (me != null) ...[
          _profile(context, me),
          const SizedBox(height: Space.l),
          Field(label: 'Friend ID', child: _friendId(context, me)),
          _requestsRow(context, me),
          const SizedBox(height: Space.l),
          if (_session?.email case final email? when email.isNotEmpty)
            Text(
              'People uses your Claktory account, $email.',
              style: type.caption.copyWith(color: palette.textFaint),
            ),
        ],
        const SizedBox(height: Space.l),
      ],
      error: _error,
      submitLabel: 'Done',
      onSubmit: widget.close,
      onCancel: widget.close,
    );
  }

  UserAppsSession? get _session => _api.session;

  Widget _profile(BuildContext context, MyIdentity me) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final editing = _name;
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        PersonAvatar(person: peopleAvatar(me), size: 40),
        const SizedBox(width: Space.m),
        Expanded(
          child: editing != null
              ? Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    TextField(
                      controller: editing,
                      autofocus: true,
                      placeholder: 'Your name',
                      semanticsLabel: 'Your name',
                      inputFormatters: [LengthLimitingTextInputFormatter(80)],
                      onSubmitted: (_) => _saveName(),
                    ),
                    const SizedBox(height: Space.s),
                    Row(
                      children: [
                        const Spacer(),
                        Button(
                          label: 'Cancel',
                          onPressed: () => setState(() {
                            editing.dispose();
                            _name = null;
                          }),
                        ),
                        const SizedBox(width: Space.s),
                        Button(
                          label: 'Save',
                          primary: true,
                          onPressed: _busy ? null : _saveName,
                        ),
                      ],
                    ),
                  ],
                )
              : Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      me.name.isEmpty ? 'No name yet' : me.name,
                      style: type.emphasis.copyWith(
                        color: me.name.isEmpty
                            ? palette.textFaint
                            : palette.text,
                      ),
                    ),
                    Text(
                      'Friends see this name and picture.',
                      style: type.caption.copyWith(color: palette.textMuted),
                    ),
                    const SizedBox(height: Space.s),
                    Wrap(
                      spacing: Space.s,
                      runSpacing: Space.s,
                      children: [
                        Button(
                          label: me.name.isEmpty
                              ? 'Add a name…'
                              : 'Change name…',
                          onPressed: _busy ? null : _editName,
                        ),
                        Button(
                          label: 'New picture',
                          onPressed: _busy ? null : _newAvatar,
                        ),
                      ],
                    ),
                  ],
                ),
        ),
      ],
    );
  }

  Widget _friendId(BuildContext context, MyIdentity me) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final code = _code;
    final mono = TextStyles.code.copyWith(color: palette.text, fontSize: 13);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (code == null)
          Text(
            // Its shape, not its letters.
            '•••• •••• •••• •••• •••• •••• •••• ••••',
            style: mono.copyWith(color: palette.textFaint),
            semanticsLabel: 'Hidden',
          )
        else
          Text(groupedCode(code.code), style: mono),
        const SizedBox(height: Space.s),
        Wrap(
          spacing: Space.s,
          runSpacing: Space.s,
          children: code == null
              ? [
                  Button(
                    label: 'Show',
                    primary: true,
                    onPressed: _busy ? null : _show,
                  ),
                ]
              : [
                  Button(
                    label: 'Copy',
                    onPressed: () =>
                        Clipboard.setData(ClipboardData(text: code.code)),
                  ),
                  Button(
                    label: _qr ? 'Hide QR code' : 'Show QR code',
                    onPressed: () => setState(() => _qr = !_qr),
                  ),
                  Button(label: 'Hide', onPressed: _hide),
                ],
        ),
        if (code != null && _qr) ...[
          const SizedBox(height: Space.m),
          QrCode(
            key: ValueKey(code.code),
            text: friendLink(code.code),
            size: 180,
          ),
        ],
        const SizedBox(height: Space.s),
        Text(
          'Share it only with people you want to hear from. If it gets to '
          'someone else, reset it.',
          style: type.caption.copyWith(color: palette.textMuted),
        ),
        const SizedBox(height: Space.s),
        Button(label: 'Reset friend ID…', onPressed: _busy ? null : _reset),
        const SizedBox(height: Space.l),
      ],
    );
  }

  Widget _requestsRow(BuildContext context, MyIdentity me) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return ToggleTap(
      value: me.requestsOn,
      onChanged: _busy ? null : _requests,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  'Friend requests',
                  style: type.body.copyWith(color: palette.text),
                ),
                Text(
                  me.requestsOn
                      ? 'Anyone with your friend ID can send you one.'
                      : 'Nobody can send you one, even with your friend ID.',
                  style: type.caption.copyWith(color: palette.textMuted),
                ),
              ],
            ),
          ),
          const SizedBox(width: Space.m),
          Toggle(
            value: me.requestsOn,
            onChanged: _busy ? null : _requests,
            label: 'Friend requests',
          ),
        ],
      ),
    );
  }
}
