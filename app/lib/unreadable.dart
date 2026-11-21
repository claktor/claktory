import 'dart:async';

import 'package:cupertino_ui/cupertino_ui.dart'
    show DefaultCupertinoLocalizations;
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import 'connections.dart';
import 'controls.dart';
import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;

/// What this device is, in the words for its store ("this Mac").
String get _thisDevice => switch (defaultTargetPlatform) {
  TargetPlatform.iOS => 'this iPhone',
  TargetPlatform.android => 'this phone',
  TargetPlatform.macOS => 'this Mac',
  _ => 'this device',
};

/// The app while what this device keeps (its connections to Claktories and
/// its account) can't be read: it says so, and the app doesn't start on
/// an empty store, which would look unpaired and then be saved over the
/// real one. Try again reads it again, as does coming back to the front
/// after a failure that may pass (the device locked); Start over clears it
/// once the person confirms. Either way, [onRead] gets the Connections
/// read, to start the app with.
class UnreadableApp extends StatelessWidget {
  const UnreadableApp({
    super.key,
    required this.storage,
    required this.failure,
    required this.onRead,
  });

  final ConnectionStorage storage;
  final ConnectionStorageFailure failure;
  final ValueChanged<Connections> onRead;

  @override
  Widget build(BuildContext context) => WidgetsApp(
    title: 'Claktory',
    color: signal,
    localizationsDelegates: const [DefaultCupertinoLocalizations.delegate],
    debugShowCheckedModeBanner: false,
    builder: (context, _) =>
        Unreadable(storage: storage, failure: failure, onRead: onRead),
  );
}

/// [_thisDevice] starting a sentence.
String get _thisDeviceStarting =>
    '${_thisDevice[0].toUpperCase()}${_thisDevice.substring(1)}';

/// [UnreadableApp]'s screen.
class Unreadable extends StatefulWidget {
  const Unreadable({
    super.key,
    required this.storage,
    required this.failure,
    required this.onRead,
  });

  final ConnectionStorage storage;
  final ConnectionStorageFailure failure;
  final ValueChanged<Connections> onRead;

  @override
  State<Unreadable> createState() => _UnreadableState();
}

class _UnreadableState extends State<Unreadable> {
  late var _failure = widget.failure;
  var _busy = false;
  var _confirming = false;

  /// Why starting over failed.
  String? _error;

  late final _lifecycle = AppLifecycleListener(
    onResume: () {
      if (_failure.transient && !_busy) unawaited(_tryAgain());
    },
  );

  @override
  void initState() {
    super.initState();
    _lifecycle;
  }

  @override
  void dispose() {
    _lifecycle.dispose();
    super.dispose();
  }

  Future<void> _tryAgain() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    final connections = await Connections.load(widget.storage);
    if (!mounted) return;
    final failure = connections.readFailure;
    if (failure == null) {
      widget.onRead(connections);
      return;
    }
    connections.dispose();
    setState(() {
      _busy = false;
      _failure = failure;
    });
  }

  Future<void> _startOver() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await widget.storage.reset();
    } on ConnectionStorageFailure catch (failure) {
      if (!mounted) return;
      setState(() {
        _busy = false;
        _error = "Couldn't start over: ${failure.message}";
      });
      return;
    }
    if (!mounted) return;
    await _tryAgain();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final device = _thisDevice;
    final said = _failure.message.isEmpty ? _failure.code : _failure.message;
    final (String title, String body, List<Widget> buttons) = _confirming
        ? (
            'Start over?',
            '$_thisDeviceStarting forgets its connections and signs out, so '
                'you connect and sign in again. Your Claktories and their '
                'agents stay as they are.',
            [
              Button(
                label: 'Cancel',
                onPressed: _busy
                    ? null
                    : () => setState(() => _confirming = false),
              ),
              Button(
                label: 'Start over',
                primary: true,
                onPressed: _busy ? null : _startOver,
              ),
            ],
          )
        : (
            "Claktory can't read $device's sign-in and connections",
            _failure.transient
                ? "$_thisDeviceStarting can't open them right now. Unlock it, then "
                      'try again.'
                : "Try again. If that doesn't work, start over, then connect "
                      'and sign in again.',
            [
              Button(
                label: 'Start over…',
                onPressed: _busy
                    ? null
                    : () => setState(() => _confirming = true),
              ),
              Button(
                label: _busy ? 'Trying…' : 'Try again',
                primary: true,
                onPressed: _busy ? null : _tryAgain,
              ),
            ],
          );
    return ColoredBox(
      color: palette.canvas,
      child: SafeArea(
        child: Center(
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(Space.xl),
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 420),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Semantics(
                    header: true,
                    child: Text(
                      title,
                      style: type.emphasis.copyWith(color: palette.text),
                    ),
                  ),
                  const SizedBox(height: Space.s),
                  Text(
                    body,
                    style: type.body.copyWith(color: palette.textMuted),
                  ),
                  if (!_confirming && said.isNotEmpty) ...[
                    const SizedBox(height: Space.s),
                    Text(
                      said,
                      style: type.caption.copyWith(color: palette.textFaint),
                    ),
                  ],
                  if (_error case final error?) ...[
                    const SizedBox(height: Space.m),
                    Text(
                      error,
                      style: type.caption.copyWith(color: palette.danger),
                    ),
                  ],
                  const SizedBox(height: Space.xl),
                  Wrap(
                    alignment: WrapAlignment.end,
                    spacing: Space.s,
                    runSpacing: Space.s,
                    children: buttons,
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// While the last change to what this device keeps couldn't be saved: a
/// line at the window's foot saying so, with Try again. What shows is
/// right; only the next launch would miss it.
class UnsavedNote extends StatelessWidget {
  const UnsavedNote({super.key, required this.connections});

  final Connections connections;

  @override
  Widget build(BuildContext context) =>
      ValueListenableBuilder<ConnectionStorageFailure?>(
        valueListenable: connections.writeFailure,
        builder: (context, failure, _) {
          if (failure == null) return const SizedBox.shrink();
          final palette = Palette.of(context);
          final type = TypeScale.of(context);
          return SafeArea(
            top: false,
            child: Padding(
              padding: const EdgeInsets.all(Space.m),
              child: Container(
                constraints: const BoxConstraints(maxWidth: 520),
                padding: const EdgeInsets.fromLTRB(
                  Space.m,
                  Space.s,
                  Space.s,
                  Space.s,
                ),
                decoration: BoxDecoration(
                  color: palette.sidebar,
                  border: Border.all(color: palette.hairline),
                  borderRadius: BorderRadius.circular(Radii.floating),
                  boxShadow: floatingShadow,
                ),
                child: Row(
                  children: [
                    Expanded(
                      child: Text(
                        "Couldn't save $_thisDevice's connections. "
                        '${failure.message}',
                        style: type.caption.copyWith(color: palette.danger),
                      ),
                    ),
                    const SizedBox(width: Space.s),
                    Button(label: 'Try again', onPressed: connections.keep),
                  ],
                ),
              ),
            ),
          );
        },
      );
}
