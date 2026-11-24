import 'dart:math' as math;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'mobile/input.dart' show InputKind;
import 'mobile/metrics.dart' show Touch, TypeScale, withoutInsets;
import 'rail.dart'
    show Claktories, ClaktoriesBackStrip, ClaktoriesScope, Claktory;
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

/// The name the person gave another Claktory this app is connected to
/// (this device's own first), so connecting one more doesn't ask again
/// from scratch; null when none knows it.
String? nameElsewhere(Claktories? claktories, Backend current) {
  for (final claktory in claktories?.all ?? const <Claktory>[]) {
    if (identical(claktory.backend, current)) continue;
    final name = claktory.backend.workspace.workspace.settings.userName.trim();
    if (name.isNotEmpty) return name;
  }
  return null;
}

/// A first name guessed from [email]'s local part, for the name step to
/// suggest: "matt.k@…" and "matt_k42@…" give "Matt". Null when nothing
/// there looks like a first name: too short, a role such as "info", a
/// word such as "test" or "claktory", or the email's own domain
/// ("acme@acme.com").
String? nameFromEmail(String email) {
  final at = email.indexOf('@');
  if (at <= 0) return null;
  final local = email.substring(0, at).split('+').first;
  final first = local
      .split(RegExp(r'[^A-Za-zÀ-ÖØ-öø-ÿ]+'))
      .firstWhere((piece) => piece.isNotEmpty, orElse: () => '');
  final lower = first.toLowerCase();
  final domain = email.substring(at + 1).toLowerCase().split('.');
  if (first.length < 2 || _notNames.contains(lower) || domain.contains(lower)) {
    return null;
  }
  // As typed when it mixes cases ("McKay"), else capitalised.
  final mixed = first != first.toLowerCase() && first != first.toUpperCase();
  return mixed
      ? '${first[0].toUpperCase()}${first.substring(1)}'
      : '${first[0].toUpperCase()}${first.substring(1).toLowerCase()}';
}

/// Local parts that name a role, a mailbox or a purpose, not a person.
const _notNames = {
  'admin',
  'claktory',
  'contact',
  'demo',
  'dev',
  'hello',
  'hi',
  'info',
  'mail',
  'me',
  'noreply',
  'no',
  'office',
  'support',
  'team',
  'test',
  'user',
};

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

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // Started from the name another Claktory here knows, all selected so
    // typing replaces it. Else the email this one signed in with may give
    // a guess, only suggested: it can be wrong.
    if (_prefilled) return;
    _prefilled = true;
    final backend = AppScope.of(context);
    final workspace = backend.workspace.workspace;
    final known = nameElsewhere(ClaktoriesScope.of(context), backend);
    if (known != null) {
      _name.value = TextEditingValue(
        text: known,
        selection: TextSelection(baseOffset: 0, extentOffset: known.length),
      );
    } else if (workspace.hasAccount()) {
      _suggestion = nameFromEmail(workspace.account.email);
    }
  }

  var _prefilled = false;

  /// A name guessed from the email, offered under the empty field.
  String? _suggestion;
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
    final phone = isPhoneLayout(context);
    final type = TypeScale.of(context);
    return StepCard(
      child: ListenableBuilder(
        listenable: _name,
        builder: (context, _) {
          final save = _name.text.trim().isEmpty || _saving ? null : _save;
          return Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Semantics(
                header: true,
                child: Text(
                  "What's your name?",
                  style: stepTitleStyle(context).copyWith(color: palette.text),
                ),
              ),
              const SizedBox(height: Space.s),
              Text(
                'Your agents will call you by it.',
                style: type.body.copyWith(color: palette.textMuted),
              ),
              const SizedBox(height: Space.xl),
              TextField(
                controller: _name,
                placeholder: 'First name',
                autofocus: true,
                kind: InputKind.name,
                scrollPadding: stepFieldScrollPadding(context),
                onSubmitted: (_) => _save(),
              ),
              if (_suggestion case final suggestion?
                  when _name.text.trim().isEmpty) ...[
                SizedBox(height: phone ? 0 : Space.s),
                Align(
                  alignment: Alignment.centerLeft,
                  child: Semantics(
                    button: true,
                    child: Pressable(
                      onTap: () => _name.value = TextEditingValue(
                        text: suggestion,
                        selection: TextSelection.collapsed(
                          offset: suggestion.length,
                        ),
                      ),
                      builder: (context, hovered) {
                        final text = Text(
                          '$suggestion?',
                          style: type.body.copyWith(
                            color: hovered ? palette.text : palette.textMuted,
                            decoration: TextDecoration.underline,
                            decorationColor: hovered
                                ? palette.text
                                : palette.textMuted,
                          ),
                        );
                        // A finger's height on a phone.
                        return phone
                            ? ConstrainedBox(
                                constraints: const BoxConstraints(
                                  minHeight: Touch.target,
                                  minWidth: Touch.target,
                                ),
                                child: Align(
                                  alignment: Alignment.centerLeft,
                                  widthFactor: 1,
                                  child: text,
                                ),
                              )
                            : text;
                      },
                    ),
                  ),
                ),
              ],
              if (_error case final error?) ...[
                const SizedBox(height: Space.s),
                Text(
                  error,
                  style: type.caption.copyWith(color: palette.danger),
                ),
              ],
              const SizedBox(height: Space.xl),
              if (phone)
                StepButtons(
                  children: [
                    Button(label: 'Continue', primary: true, onPressed: save),
                  ],
                )
              else
                Align(
                  alignment: Alignment.centerRight,
                  child: Button(
                    label: 'Continue',
                    primary: true,
                    onPressed: save,
                  ),
                ),
            ],
          );
        },
      ),
    );
  }
}

/// A first-launch step's card, over the whole window: a 400px floating
/// card on a 40% black scrim, scrolling when the window is too short.
///
/// On a phone, a page of its own instead: the way back to the
/// Claktories along the top when there's a list to go back to, then the
/// step scrolling between the safe areas and above the keyboard, so what's
/// typed and the button under it stay in view.
class StepCard extends StatelessWidget {
  const StepCard({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    if (isPhoneLayout(context)) return _page(context, palette);
    // Centred in what an on-screen keyboard leaves (an Android tablet's,
    // an iPad's), so the field and the button under it stay in sight.
    final safe = MediaQuery.paddingOf(context).bottom;
    final keyboard = MediaQuery.viewInsetsOf(context).bottom;
    return ColoredBox(
      color: const Color(0xFF000000).withValues(alpha: 0.4),
      child: SafeArea(
        bottom: false,
        child: Padding(
          padding: EdgeInsets.only(bottom: math.max(safe, keyboard)),
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
      ),
    );
  }

  Widget _page(BuildContext context, Palette palette) {
    final safe = MediaQuery.paddingOf(context);
    final keyboard = MediaQuery.viewInsetsOf(context).bottom;
    return ColoredBox(
      color: palette.canvas,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // The strip, which keeps clear of the status bar itself, or
          // where there's none, room for the status bar.
          Stack(
            children: [
              SizedBox(height: safe.top),
              const ClaktoriesBackStrip(),
            ],
          ),
          Expanded(
            child: Padding(
              padding: EdgeInsets.only(
                left: safe.left,
                right: safe.right,
                bottom: math.max(safe.bottom, keyboard),
              ),
              child: withoutInsets(
                context,
                child: SingleChildScrollView(
                  padding: const EdgeInsets.fromLTRB(
                    Touch.gutter,
                    Space.xxl,
                    Touch.gutter,
                    Space.xl,
                  ),
                  child: Center(
                    child: ConstrainedBox(
                      constraints: const BoxConstraints(maxWidth: 420),
                      child: child,
                    ),
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// How much room a step's field keeps around it when it scrolls into view:
/// on a phone, enough under it that the step's button stays above the
/// keyboard too.
EdgeInsets stepFieldScrollPadding(BuildContext context) =>
    isPhoneLayout(context)
    ? const EdgeInsets.fromLTRB(20, 20, 20, 160)
    : const EdgeInsets.all(20);

/// A step's heading: emphasis in its card, a page's title on a phone.
TextStyle stepTitleStyle(BuildContext context) =>
    isPhoneLayout(context) ? TypeScale.of(context).title : TextStyles.emphasis;

/// A step's buttons on a phone, under what they act on: the step's action
/// first, then the others, each the page's width and a finger tall.
class StepButtons extends StatelessWidget {
  const StepButtons({super.key, required this.children});

  /// The primary first.
  final List<Widget> children;

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      for (final (index, button) in children.indexed) ...[
        if (index > 0) const SizedBox(height: Space.s),
        button,
      ],
    ],
  );
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
