import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';

/// Whether to ask the person's name: the backend is connected and has said
/// what it knows, and it doesn't know their name (`Settings.user_name`). A
/// phone not yet paired (an empty workspace, never connected) isn't asked.
bool asksName(BuildContext context) {
  final backend = AppScope.of(context);
  final store = backend.workspace;
  return backend.connected.value &&
      store.received &&
      store.workspace.settings.userName.trim().isEmpty;
}

/// The first launch's one question, over the window until it's answered:
/// the person's first name, which agents call them by. It's the backend's
/// (`Settings.user_name`), so it's asked once, on any device.
class NameStep extends StatefulWidget {
  const NameStep({super.key});

  @override
  State<NameStep> createState() => _NameStepState();
}

class _NameStepState extends State<NameStep> {
  final _name = TextEditingController();
  var _saving = false;
  String? _error;

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    final name = _name.text.trim();
    if (name.isEmpty || _saving) return;
    setState(() {
      _saving = true;
      _error = null;
    });
    try {
      await AppScope.of(context).updateSettings(UpdateSettings(userName: name));
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return StepCard(
      child: ListenableBuilder(
        listenable: _name,
        builder: (context, _) => Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              "What's your name?",
              style: TextStyles.emphasis.copyWith(color: palette.text),
            ),
            const SizedBox(height: Space.s),
            Text(
              'Your agents will call you by it.',
              style: TextStyles.body.copyWith(color: palette.textMuted),
            ),
            const SizedBox(height: Space.xl),
            TextField(
              controller: _name,
              placeholder: 'First name',
              autofocus: true,
              onSubmitted: (_) => _save(),
            ),
            if (_error case final error?) ...[
              const SizedBox(height: Space.s),
              Text(
                error,
                style: TextStyles.caption.copyWith(color: palette.danger),
              ),
            ],
            const SizedBox(height: Space.xl),
            Align(
              alignment: Alignment.centerRight,
              child: Button(
                label: 'Continue',
                primary: true,
                onPressed: _name.text.trim().isEmpty || _saving ? null : _save,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// A first-launch step's card, over the whole window: a 400px floating
/// card on a 40% black scrim, scrolling when the window is too short.
class StepCard extends StatelessWidget {
  const StepCard({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ColoredBox(
      color: const Color(0xFF000000).withValues(alpha: 0.4),
      child: SafeArea(
        child: Center(
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(Space.l),
            child: Container(
              width: 400,
              padding: const EdgeInsets.all(Space.xl),
              decoration: BoxDecoration(
                color: palette.sidebar,
                borderRadius: BorderRadius.circular(Radii.floating),
                border: Border.all(color: palette.hairline),
                boxShadow: floatingShadow,
              ),
              child: child,
            ),
          ),
        ),
      ),
    );
  }
}

/// Keeps Esc and the like from reaching what's under the step.
class NameStepBarrier extends StatelessWidget {
  const NameStepBarrier({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) => Shortcuts(
    shortcuts: const <ShortcutActivator, Intent>{
      SingleActivator(LogicalKeyboardKey.escape): DoNothingIntent(),
    },
    child: child,
  );
}
