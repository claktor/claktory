import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../devices.dart' show QrCode;
import '../forms.dart' show FormFrame;
import '../identity.dart';
import '../mobile/metrics.dart' show TypeScale;
import '../scanner.dart';
import '../scope.dart';
import '../visible_codes.dart' show nameInText;
import 'e2ee.dart';

// E5, Verify (dms-e2ee §2): the security code two friends
// compare, in person or on a call, or by one scanning the other's QR
// code; matching, the person marks the friend verified, until their
// security key changes.

/// Opens Verify for [account], called [name], over what's showing.
Future<void> showVerify(
  BuildContext context,
  E2ee e2ee, {
  required String account,
  required String name,
}) => showModal(
  context,
  (context, close) =>
      VerifyView(e2ee: e2ee, account: account, name: name, close: close),
);

class VerifyView extends StatefulWidget {
  const VerifyView({
    super.key,
    required this.e2ee,
    required this.account,
    required this.name,
    required this.close,
  });

  final E2ee e2ee;
  final String account;
  final String name;
  final VoidCallback close;

  @override
  State<VerifyView> createState() => _VerifyViewState();
}

class _VerifyViewState extends State<VerifyView> with WidgetsBindingObserver {
  CameraAccess? _access;
  var _scanning = false;

  /// What the last scan said: true for a match, false for one that
  /// doesn't, null for none yet.
  bool? _matched;
  String? _error;
  var _busy = false;

  /// The person said the numbers don't match and the key change was
  /// undone (E11-3).
  var _undone = false;

  // Their name inside a sentence: shown so it can't reorder the words
  // around it (nameInText).
  String get _name => nameInText(widget.name);
  String get _them => widget.name.isEmpty ? 'them' : _name;
  String get _their => widget.name.isEmpty ? 'their' : "$_name's";

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    unawaited(_ask());
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  // Back from Settings, the camera may be allowed now.
  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) unawaited(_ask());
  }

  Future<void> _ask() async {
    final access = await AppScope.scope(context).camera.access();
    if (mounted) setState(() => _access = access);
  }

  Future<void> _scan() async {
    final camera = AppScope.scope(context).camera;
    if (_access == CameraAccess.undetermined) {
      await camera.request();
      await _ask();
    }
    if (mounted && _access == CameraAccess.granted) {
      setState(() => _scanning = true);
    }
  }

  void _scanned(String code) {
    if (!_scanning) return;
    final matches = widget.e2ee.safetyMatches(widget.account, code);
    setState(() {
      _scanning = false;
      _matched = matches ?? false;
      _error = matches == null ? "That isn't a security code." : null;
    });
    // The same two keys: that's what marking says.
    if (matches == true) unawaited(_mark());
  }

  Future<void> _mark() async {
    setState(() => _busy = true);
    await widget.e2ee.verify(widget.account);
    widget.close();
  }

  /// The numbers don't match: the change to their key isn't confirmed,
  /// and the key before it is kept (E11-3).
  Future<void> _undo() async {
    setState(() => _busy = true);
    final undone = await widget.e2ee.undoKeyChange(widget.account);
    if (!mounted) return;
    // The service still gives the key that didn't match: its note is back
    // in the chat, which says so.
    if (undone && (widget.e2ee.pin(widget.account)?.changedUnseen ?? false)) {
      widget.close();
      return;
    }
    setState(() {
      _busy = false;
      _undone = undone;
      _error = undone
          ? null
          : "Couldn't go back to $_their earlier security key. Try again.";
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final safety = widget.e2ee.safety(widget.account);
    final pin = widget.e2ee.pin(widget.account);
    final verified = pin?.verified ?? false;
    // A change the person can still say no to: the key before it is kept.
    final canUndo = pin?.changedFrom != null && !verified && !_undone;
    final groups = safety?.digits.split(' ') ?? const <String>[];
    // Scan where the camera works or can be asked for; refused, how to
    // allow it; none, or one the person can't allow (Screen Time), no
    // Scan at all: the numbers are compared by eye.
    final canScan =
        _access == CameraAccess.granted || _access == CameraAccess.undetermined;
    final refused = _access == CameraAccess.denied;
    return FormFrame(
      title: 'Verify ${widget.name.isEmpty ? 'your friend' : _name}',
      body:
          "Compare these numbers with the ones on $_their device, in person "
          "or on a call, or scan the code on $_their device.",
      fields: [
        if (verified)
          Padding(
            padding: const EdgeInsets.only(bottom: Space.s),
            child: Text(
              'Verified',
              style: type.emphasis.copyWith(color: palette.signal),
            ),
          ),
        if (safety != null) ...[
          // Three rows of four groups, as both devices show them.
          for (var row = 0; row < groups.length; row += 4)
            Text(
              groups.skip(row).take(4).join('  '),
              style: type.emphasis.copyWith(
                color: palette.text,
                fontFeatures: const [FontFeature.tabularFigures()],
                letterSpacing: 0.5,
              ),
            ),
          const SizedBox(height: Space.m),
          if (_scanning)
            AspectRatio(
              aspectRatio: 1,
              child: Container(
                clipBehavior: Clip.antiAlias,
                decoration: BoxDecoration(
                  color: const Color(0xFF000000),
                  borderRadius: BorderRadius.circular(Radii.panel),
                  border: Border.all(color: palette.hairline),
                ),
                child: AppScope.scope(context).camera.preview(_scanned),
              ),
            )
          else
            Center(
              child: QrCode(
                text: '$dmSafetyPrefix${dmB64(safety.qr)}',
                size: 200,
              ),
            ),
        ] else
          Text(
            "There's no security code with $_them yet.",
            style: type.body.copyWith(color: palette.textMuted),
          ),
        if (_matched == false && _error == null) ...[
          const SizedBox(height: Space.s),
          Text(
            "These don't match. Don't mark $_them as verified.",
            style: type.body.copyWith(color: palette.danger),
          ),
        ],
        if (_undone) ...[
          const SizedBox(height: Space.s),
          Text(
            "Claktory kept $_their earlier security key. If the new one comes "
            "back, this chat will say it's the one that didn't match.",
            style: type.body.copyWith(color: palette.text),
          ),
        ] else if (canUndo && safety != null && !_scanning) ...[
          const SizedBox(height: Space.s),
          Text(
            "${widget.name.isEmpty ? 'Their' : _their} security key changed. "
            "If these numbers don't match the "
            "ones on $_their device, don't accept the change.",
            style: type.body.copyWith(color: palette.textMuted),
          ),
          const SizedBox(height: Space.s),
          Align(
            alignment: AlignmentDirectional.centerStart,
            child: Button(
              label: "The numbers don't match",
              onPressed: _busy ? null : _undo,
            ),
          ),
        ],
        if (canScan && safety != null && !_scanning && !_undone) ...[
          const SizedBox(height: Space.s),
          Align(
            alignment: AlignmentDirectional.centerStart,
            child: Button(label: "Scan $_their code", onPressed: _scan),
          ),
        ],
        if (refused && safety != null) ...[
          const SizedBox(height: Space.s),
          Text(
            defaultTargetPlatform == TargetPlatform.android
                ? 'To scan $_their code, allow Claktory to use the camera in '
                      'Settings.'
                : 'To scan $_their code, allow camera access in Settings → '
                      'Claktory.',
            style: type.body.copyWith(color: palette.textMuted),
          ),
          const SizedBox(height: Space.s),
          Align(
            alignment: AlignmentDirectional.centerStart,
            child: Button(
              label: 'Open Settings',
              onPressed: () =>
                  unawaited(AppScope.scope(context).camera.openSettings()),
            ),
          ),
        ],
      ],
      error: _error,
      submitLabel: 'Mark as verified',
      onSubmit:
          safety == null || verified || _busy || _matched == false || _undone
          ? null
          : _mark,
      cancelLabel: verified || _undone ? 'Done' : 'Cancel',
      onCancel: widget.close,
    );
  }
}
