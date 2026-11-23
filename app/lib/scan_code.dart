import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart' show fireAndLog;
import 'controls.dart';
import 'devices.dart' show thisDeviceKind;
import 'identity.dart';
import 'mobile/haptics.dart';
import 'mobile/header.dart';
import 'mobile/metrics.dart';
import 'mobile/navigation.dart';
import 'mobile/presentation.dart' show PhoneSheet, presentSheet;
import 'scanner.dart';
import 'scope.dart';

/// Whether a field for a code (a pairing code, a pipe's invite, a move's
/// destination) offers Scan beside Paste (friend bug #2): wherever there's
/// a camera, even one refused so far, since the scanner then says how to
/// allow it, with Open Settings. Not where there's none (a Mac, the
/// Simulator) or where the person can't allow it (Screen Time, a managed
/// phone).
Future<bool> canOfferScan(BuildContext context) async {
  final access = await AppScope.scope(context).camera.access();
  return access != CameraAccess.unavailable &&
      access != CameraAccess.restricted;
}

/// Scans a code's QR code with the camera: on a phone a page that rises
/// from the bottom, titled [title], with Close; elsewhere (an iPad) a
/// dialog. Each code the camera reads goes to [check], which says why it
/// can't be used (shown under the picture, the camera reading on) or null
/// when it can: then it closes with that code, trimmed. Null when closed
/// without one. A camera not asked for yet is asked for as it opens (the
/// person just tapped Scan); a refused one says how to allow it, with Open
/// Settings, and the camera starts once it's allowed and the person comes
/// back.
Future<String?> scanCode(
  BuildContext context, {
  required String title,
  required String? Function(String text) check,
}) async {
  // The keyboard goes down: the camera's picture needs the room.
  FocusManager.instance.primaryFocus?.unfocus();
  final routes = PhoneRoutes.of(context);
  if (isPhoneLayout(context)) {
    // Asked from a sheet (Your Claktories, Move…), a page would open under
    // it: the scanner rises over it as a sheet of its own, as tall.
    if (routes == null ||
        context.findAncestorWidgetOfExactType<PhoneSheet>() != null) {
      return presentSheet<String>(
        context,
        title: title,
        tall: true,
        builder: (context, close) => _Scanner(
          title: title,
          check: check,
          frame: _Frame.sheet,
          done: ([code]) => close(code),
        ),
      );
    }
    return routes.push<String>(
      PhoneRoute(
        id: 'scan-code',
        fullscreenDialog: true,
        builder: (context) => _Scanner(
          title: title,
          check: check,
          frame: _Frame.page,
          done: ([code]) => routes.pop<String>(code),
        ),
      ),
    );
  }
  String? scanned;
  await showModal(
    context,
    (context, close) => _Scanner(
      title: title,
      check: check,
      frame: _Frame.dialog,
      done: ([code]) {
        scanned = code;
        close();
      },
    ),
  );
  return scanned;
}

/// Where the scanner shows.
enum _Frame {
  /// A phone's page, with its own header and Close.
  page,

  /// A phone's sheet, whose title and Close are the sheet's.
  sheet,

  /// The inside of a dialog (an iPad), with its title and Cancel.
  dialog,
}

class _Scanner extends StatefulWidget {
  const _Scanner({
    required this.title,
    required this.check,
    required this.frame,
    required this.done,
  });

  final String title;
  final String? Function(String text) check;
  final _Frame frame;

  /// Closes it, with the code when there's one.
  final void Function([String? code]) done;

  @override
  State<_Scanner> createState() => _ScannerState();
}

class _ScannerState extends State<_Scanner> with WidgetsBindingObserver {
  /// Whether the camera may be used; null until known.
  CameraAccess? _access;

  /// Why the last code read can't be used.
  String? _problem;

  /// A code was taken: what the camera reads next is ignored.
  var _taken = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    WidgetsBinding.instance.addPostFrameCallback((_) => _check(ask: true));
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  // Back from Settings, the camera may be allowed now.
  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) unawaited(_check());
  }

  /// Finds out whether the camera may be used, asking the person ([ask])
  /// when they haven't answered yet. Refused, it's asked again rather than
  /// assumed for good: Android can still ask after a first no.
  Future<void> _check({bool ask = false}) async {
    if (!mounted) return;
    final camera = AppScope.scope(context).camera;
    var access = await camera.access();
    if (!mounted) return;
    if (ask && access == CameraAccess.undetermined) {
      access = await camera.request()
          ? CameraAccess.granted
          : await camera.access();
    }
    if (mounted && access != _access) setState(() => _access = access);
  }

  void _scanned(String code) {
    if (_taken) return;
    final text = code.trim();
    final problem = widget.check(text);
    if (problem != null) {
      if (problem != _problem) setState(() => _problem = problem);
      return;
    }
    _taken = true;
    unawaited(Haptics.success());
    widget.done(text);
  }

  void _close() => widget.done();

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final body = Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (widget.frame == _Frame.dialog) ...[
          Text(
            widget.title,
            style: TypeScale.of(context).emphasis.copyWith(color: palette.text),
          ),
          const SizedBox(height: Space.l),
        ],
        _content(context, palette),
      ],
    );
    if (widget.frame != _Frame.page) return body;
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): _close},
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          PhoneHeader(
            title: Text(
              widget.title,
              style: PhoneType.emphasis.copyWith(color: palette.text),
            ),
            trailing: [HeaderButton.close(onTap: _close)],
            search: false,
          ),
          Expanded(
            child: SingleChildScrollView(
              padding: const EdgeInsets.all(Touch.gutter),
              child: body,
            ),
          ),
        ],
      ),
    );
  }

  Widget _content(BuildContext context, Palette palette) {
    final access = _access;
    final type = TypeScale.of(context);
    // Back to the field, where the code can be pasted instead; a dialog
    // has Cancel instead.
    Widget back(String label) => Align(
      alignment: AlignmentDirectional.centerEnd,
      child: Button(label: label, onPressed: _close),
    );
    switch (access) {
      case null:
        // Finding out whether there's a camera.
        return const SizedBox(height: 120);
      case CameraAccess.granted:
        final problem = _problem;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // Square, and no more than half the screen's height, so what's
            // under it stays in view.
            Center(
              child: ConstrainedBox(
                constraints: BoxConstraints(
                  maxHeight: MediaQuery.sizeOf(context).height / 2,
                ),
                child: AspectRatio(
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
                ),
              ),
            ),
            if (problem != null) ...[
              const SizedBox(height: Space.s),
              Text(
                problem,
                style: type.caption.copyWith(color: palette.danger),
              ),
            ],
            if (widget.frame == _Frame.dialog) ...[
              const SizedBox(height: Space.l),
              back('Cancel'),
            ],
          ],
        );
      case CameraAccess.undetermined:
        // Asked, and the question put away without an answer (Android):
        // the person asks again.
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Button(
              label: 'Scan code',
              primary: true,
              onPressed: () => _check(ask: true),
            ),
            const SizedBox(height: Space.m),
            back(widget.frame == _Frame.dialog ? 'Cancel' : 'Back'),
          ],
        );
      case CameraAccess.denied ||
          CameraAccess.restricted ||
          CameraAccess.unavailable:
        final refused = access == CameraAccess.denied;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              !refused
                  ? "The camera isn't available on this $thisDeviceKind. "
                        'Paste the code instead.'
                  : defaultTargetPlatform == TargetPlatform.android
                  ? 'To scan, allow Claktory to use the camera in Settings.'
                  : 'To scan, allow camera access in Settings → Claktory.',
              style: type.body.copyWith(color: palette.text),
            ),
            const SizedBox(height: Space.l),
            Wrap(
              alignment: WrapAlignment.end,
              spacing: Space.s,
              runSpacing: Space.s,
              children: [
                Button(label: 'Back', onPressed: _close),
                if (refused)
                  Button(
                    label: 'Open Settings',
                    primary: true,
                    onPressed: () => fireAndLog(
                      AppScope.scope(context).camera.openSettings(),
                    ),
                  ),
              ],
            ),
          ],
        );
    }
  }
}
