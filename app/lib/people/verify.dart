import 'dart:async';

import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../devices.dart' show QrCode;
import '../forms.dart' show FormFrame;
import '../identity.dart';
import '../mobile/metrics.dart' show TypeScale;
import '../scanner.dart';
import '../scope.dart';
import 'e2ee.dart';

// E5, Verify (dms-e2ee §2, Trish's words): the security code two friends
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

class _VerifyViewState extends State<VerifyView> {
  CameraAccess? _access;
  var _scanning = false;

  /// What the last scan said: true for a match, false for one that
  /// doesn't, null for none yet.
  bool? _matched;
  String? _error;
  var _busy = false;

  String get _them => widget.name.isEmpty ? 'them' : widget.name;
  String get _their => widget.name.isEmpty ? 'their' : "${widget.name}'s";

  @override
  void initState() {
    super.initState();
    unawaited(_ask());
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

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final safety = widget.e2ee.safety(widget.account);
    final verified = widget.e2ee.pin(widget.account)?.verified ?? false;
    final groups = safety?.digits.split(' ') ?? const <String>[];
    final canScan =
        _access != null &&
        _access != CameraAccess.unavailable &&
        _access != CameraAccess.denied;
    return FormFrame(
      title: 'Verify ${widget.name.isEmpty ? 'your friend' : widget.name}',
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
        if (canScan && safety != null && !_scanning) ...[
          const SizedBox(height: Space.s),
          Align(
            alignment: AlignmentDirectional.centerStart,
            child: Button(label: "Scan $_their code", onPressed: _scan),
          ),
        ],
      ],
      error: _error,
      submitLabel: 'Mark as verified',
      onSubmit: safety == null || verified || _busy || _matched == false
          ? null
          : _mark,
      cancelLabel: verified ? 'Done' : 'Cancel',
      onCancel: widget.close,
    );
  }
}
