import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'devices.dart' show thisDeviceKind;
import 'identity.dart';
import 'mobile/device_permissions.dart' show CameraPermission;
import 'mobile/haptics.dart';
import 'mobile/header.dart';
import 'mobile/metrics.dart';
import 'mobile/navigation.dart';
import 'mobile/permission.dart'
    show PermissionAccess, PermissionContent, PermissionFlow, PermissionWayOut;
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

/// A scanner's Paste code instead for a field with no Paste of its own:
/// what's on the clipboard goes in [field], then [then] (reading it as a
/// scanned code is read). Nothing on the clipboard: the field has the keys,
/// for the person to paste or type.
VoidCallback pasteCodeInto(TextEditingController field, {VoidCallback? then}) =>
    () => unawaited(() async {
      final text = (await Clipboard.getData(Clipboard.kTextPlain))?.text
          ?.trim();
      if (text == null || text.isEmpty) return;
      field.text = text;
      then?.call();
    }());

/// Scans a code's QR code with the camera: on a phone a page that rises
/// from the bottom, titled [title], with Close; elsewhere (an iPad) a
/// dialog. Each code the camera reads goes to [check], which says why it
/// can't be used (shown under the picture, the camera reading on) or null
/// when it can: then it closes with that code, trimmed. Null when closed
/// without one. A camera not asked for yet says why first, with Continue,
/// which brings the system's prompt (area 5a, P7); a refused one says how
/// to allow it, with Open Settings, and the camera starts once it's
/// allowed and the person comes back.
///
/// [onPasteInstead]: the field's own Paste. Given, the scanner's way out
/// (camera refused, or not available in this window) is Paste code
/// instead: it closes, puts the keys back in the field it was opened from,
/// and pastes.
Future<String?> scanCode(
  BuildContext context, {
  required String title,
  required String? Function(String text) check,
  VoidCallback? onPasteInstead,
}) async {
  // The keyboard goes down: the camera's picture needs the room. The field
  // it was opened from gets the keys back to paste instead.
  final field = FocusManager.instance.primaryFocus;
  field?.unfocus();
  final pasteInstead = onPasteInstead == null
      ? null
      : () => WidgetsBinding.instance.addPostFrameCallback((_) {
          if (field?.context?.mounted ?? false) field!.requestFocus();
          onPasteInstead();
        });
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
          pasteInstead: pasteInstead,
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
          pasteInstead: pasteInstead,
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
      pasteInstead: pasteInstead,
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
    this.pasteInstead,
  });

  final String title;
  final String? Function(String text) check;
  final _Frame frame;

  /// Pastes in the field it was opened from, once it's closed; null: none.
  final VoidCallback? pasteInstead;

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

  /// The camera is kept from the app now ([CameraAvailability]).
  var _unavailable = false;

  /// A code was taken: what the camera reads next is ignored.
  var _taken = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    WidgetsBinding.instance.addPostFrameCallback((_) => _check());
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

  /// Finds out whether the camera may be used, asking nothing: the
  /// permission's own screen asks, after saying why ([_permission]).
  Future<void> _check() async {
    if (!mounted) return;
    final access = await AppScope.scope(context).camera.access();
    if (!mounted || access == _access) return;
    setState(() {
      _access = access;
      if (access != CameraAccess.undetermined &&
          access != CameraAccess.denied) {
        _flow = null;
      }
    });
  }

  /// The camera's permission, while the scanner says why or that it's off.
  PermissionFlow? _flow;

  /// Before the camera's first use, why, with Continue; refused, how to
  /// turn it on, with Open Settings and the way out: Paste code instead
  /// where the field can, else Not now. Closes itself once allowed.
  Widget _permission(BuildContext context, CameraAccess access) {
    final flow = _flow ??= PermissionFlow(
      CameraPermission(
        AppScope.scope(context).camera,
        pairing: false,
        paste: widget.pasteInstead != null,
      ),
      from: access == CameraAccess.denied
          ? PermissionAccess.denied
          : PermissionAccess.undetermined,
    );
    final pasteInstead = widget.pasteInstead;
    return PermissionContent(
      key: ObjectKey(flow),
      flow: flow,
      fill: widget.frame == _Frame.page,
      wayOut: pasteInstead == null
          ? null
          : PermissionWayOut('Paste code instead', pasteInstead),
      onDone: () {
        final allowed = flow.result == PermissionAccess.granted;
        _flow = null;
        if (allowed) {
          unawaited(_check());
        } else {
          _close();
        }
      },
    );
  }

  /// The camera's permission is on screen: its own words, no title over
  /// them.
  bool get _asking =>
      _access == CameraAccess.undetermined || _access == CameraAccess.denied;

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
        if (widget.frame == _Frame.dialog && !_asking) ...[
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
            // The camera's permission fills the page, the way on at its
            // foot; the scanner scrolls.
            child: _asking
                ? Padding(
                    padding: const EdgeInsets.all(Touch.gutter),
                    child: _content(context, palette),
                  )
                : SingleChildScrollView(
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
        final pasteInstead = widget.pasteInstead;
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
                    // The camera kept from the app (an iPad sharing its
                    // screen, another app using it): its words over the
                    // picture, and Paste code instead below (Sorbet's).
                    child: NotificationListener<CameraAvailability>(
                      onNotification: (notice) {
                        final unavailable = notice.unavailable != null;
                        if (unavailable != _unavailable) {
                          setState(() => _unavailable = unavailable);
                        }
                        return true;
                      },
                      child: AppScope.scope(context).camera.preview(_scanned),
                    ),
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
            if (_unavailable && pasteInstead != null) ...[
              const SizedBox(height: Space.l),
              Wrap(
                alignment: WrapAlignment.end,
                spacing: Space.s,
                runSpacing: Space.s,
                children: [
                  if (widget.frame == _Frame.dialog)
                    Button(label: 'Cancel', onPressed: _close),
                  Button(
                    label: 'Paste code instead',
                    primary: true,
                    onPressed: () {
                      _close();
                      pasteInstead();
                    },
                  ),
                ],
              ),
            ] else if (widget.frame == _Frame.dialog) ...[
              const SizedBox(height: Space.l),
              back('Cancel'),
            ],
          ],
        );
      case CameraAccess.undetermined || CameraAccess.denied:
        return _permission(context, access);
      case CameraAccess.restricted || CameraAccess.unavailable:
        final pasteInstead = widget.pasteInstead;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              "The camera isn't available on this $thisDeviceKind. "
              'Paste the code instead.',
              style: type.body.copyWith(color: palette.text),
            ),
            const SizedBox(height: Space.l),
            Wrap(
              alignment: WrapAlignment.end,
              spacing: Space.s,
              runSpacing: Space.s,
              children: [
                Button(label: 'Back', onPressed: _close),
                if (pasteInstead != null)
                  Button(
                    label: 'Paste code instead',
                    primary: true,
                    onPressed: () {
                      _close();
                      pasteInstead();
                    },
                  ),
              ],
            ),
          ],
        );
    }
  }
}
