import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'claktory_identity.dart' show claktoryNameInText;
import 'controls.dart';
import 'forms.dart';
import 'identity.dart';
import 'message_time.dart' show dayLine;
import 'mobile/metrics.dart' show TypeScale;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'settings_view.dart' show SettingRow;
import 'visible_codes.dart' show visibleCodes;

/// Saving the recovery key (R1): Needs you asks once a Claktory signs in,
/// until the person shows that they kept it by typing back one block of
/// it, chosen at random by the backend each time. Whether it's confirmed
/// is the backend's (VaultState.recovery_key_unconfirmed), for every
/// device; the app keeps nothing, and the key only shows when asked.
class SaveRecoveryKeyForm extends StatefulWidget {
  const SaveRecoveryKeyForm({super.key, required this.close});

  final VoidCallback close;

  @override
  State<SaveRecoveryKeyForm> createState() => _SaveRecoveryKeyFormState();
}

class _SaveRecoveryKeyFormState extends State<SaveRecoveryKeyForm> {
  /// The key, once the person asked to see it.
  String? _key;

  /// The block to type back, once the person said they saved it.
  RecoveryKeyCheck? _check;
  final _typed = TextEditingController();
  var _busy = false;
  var _copied = false;
  String? _error;

  @override
  void dispose() {
    _typed.dispose();
    super.dispose();
  }

  Future<void> _run(Future<void> Function(Backend backend) action) async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await action(AppScope.of(context));
      if (mounted) setState(() => _busy = false);
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = failure.message;
        });
      }
    }
  }

  Future<void> _show() => _run((backend) async {
    final key = await backend.showRecoveryKey();
    if (mounted) setState(() => _key = key);
  });

  /// A fresh block to type back: each time the person comes to it.
  Future<void> _ask() => _run((backend) async {
    final check = await backend.checkRecoveryKey();
    if (!mounted) return;
    _typed.clear();
    setState(() {
      // Out of sight once they're checking what they saved.
      _key = null;
      _check = check;
    });
  });

  Future<void> _confirm() async {
    final check = _check;
    if (check == null || _typed.text.trim().isEmpty) return;
    await _run((backend) async {
      await backend.confirmRecoveryKey(check.checkId, _typed.text);
      widget.close();
    });
  }

  @override
  Widget build(BuildContext context) {
    final check = _check;
    if (check != null) return _checking(check);
    final key = _key;
    final name = claktoryNameInText(AppScope.of(context));
    return FormFrame(
      title: 'Save the recovery key for $name',
      body:
          'Each Claktory has its own recovery key, even on the same account: '
          'this one opens the secrets on $name, and no other key does.\n\n'
          'Those secrets open with two halves together: one on $name and one '
          'at claktory.com. This key opens them by itself, if claktory.com '
          'can\'t be reached, your account is gone or the half on $name is '
          'lost.\n\n'
          'Save it in a password manager, such as Passwords or 1Password, or '
          'on paper kept somewhere safe. Not in a note, a screenshot or a '
          'message. Never share it: anyone with it can open these secrets.',
      fields: [
        if (key != null) ...[
          RecoveryKeyBox(key),
          const SizedBox(height: Space.s),
          Align(
            alignment: AlignmentDirectional.centerStart,
            child: Button(
              label: _copied ? 'Copied' : 'Copy',
              onPressed: () {
                Clipboard.setData(ClipboardData(text: key));
                setState(() => _copied = true);
              },
            ),
          ),
          const SizedBox(height: Space.l),
        ] else
          Padding(
            padding: const EdgeInsets.only(bottom: Space.l),
            child: Align(
              alignment: AlignmentDirectional.centerStart,
              child: Button(
                label: 'Show recovery key',
                onPressed: _busy ? null : _show,
              ),
            ),
          ),
      ],
      error: _error,
      submitLabel: "I've saved it",
      onSubmit: _busy ? null : _ask,
      cancelLabel: 'Not now',
      onCancel: widget.close,
    );
  }

  Widget _checking(RecoveryKeyCheck check) => FormFrame(
    title: 'Check the recovery key',
    body:
        'Type block ${check.group} from the key you saved, counting the '
        'blocks of four from the left.',
    fields: [
      Field(
        label: 'Block ${check.group}',
        child: TextField(
          controller: _typed,
          autofocus: true,
          autocorrect: false,
          textCapitalization: TextCapitalization.characters,
          onSubmitted: (_) => _confirm(),
        ),
      ),
    ],
    error: _error,
    submitLabel: _busy ? 'Checking…' : 'Confirm',
    onSubmit: _busy ? null : _confirm,
    cancelLabel: 'Back',
    onCancel: () => setState(() {
      _check = null;
      _error = null;
    }),
  );
}

/// The key as a code block shows it, but wrapping at its blocks rather
/// than scrolling sideways: all of it in sight, to copy down by hand.
class RecoveryKeyBox extends StatelessWidget {
  const RecoveryKeyBox(this.text, {super.key});

  final String text;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.m,
        vertical: Space.s + 2,
      ),
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
      ),
      child: Text(
        text,
        style: TypeScale.of(context).code.copyWith(color: palette.text),
      ),
    );
  }
}

/// Who made an account recovery key, as claktory.com says (VaultState
/// recovery_key_made_on, _change_made_on): untrusted words on one line,
/// every hidden character by its code, cut at 60 characters whatever
/// claktory.com's own cap (Fugo 50510); empty, a Claktory since removed.
String recoveryKeyMaker(String name) {
  if (name.trim().isEmpty) return 'a Claktory no longer on your account';
  final shown = visibleCodes(name.replaceAll(RegExp(r'\s+'), ' ').trim());
  final runes = shown.runes;
  return runes.length <= 60
      ? shown
      : '${String.fromCharCodes(runes.take(59))}…';
}

/// The account's recovery key in Settings → Secrets (#239, main's half):
/// "Covered by your account's recovery key" with where and when it was
/// made, when it covers this vault; and when claktory.com has a new key
/// this Claktory couldn't check by itself, the warning Claktory's alert
/// sends the person here for, with Use the key here (UseRecoveryKey).
/// Nothing for a vault with its own key: Account's Show recovery key… is
/// that one's. Never Make a new recovery key, which comes with v0.2.0.
class AccountRecoveryKey extends StatefulWidget {
  const AccountRecoveryKey({super.key});

  @override
  State<AccountRecoveryKey> createState() => _AccountRecoveryKeyState();
}

class _AccountRecoveryKeyState extends State<AccountRecoveryKey> {
  var _busy = false;
  String? _error;

  Future<void> _use() async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).useRecoveryKey();
      if (mounted) setState(() => _busy = false);
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = failure.message;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final vault = backend.workspace.workspace.vault;
        if (!vault.recoveryKeyCovered && !vault.recoveryKeyChangeWaiting) {
          return const SizedBox.shrink();
        }
        final palette = Palette.of(context);
        final type = TypeScale.of(context);
        final now = DateTime.now();
        String made(String on, int ms) => [
          'Made on ${recoveryKeyMaker(on)}',
          // "today", "yesterday", "Monday", "Mon 17 Nov" (dayLine's).
          if (ms > 0)
            switch (dayLine(ms, now)) {
              'Today' || 'Yesterday' => dayLine(ms, now).toLowerCase(),
              final day => day,
            },
        ].join(', ');
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (vault.recoveryKeyChangeWaiting) ...[
              Container(
                key: const ValueKey('recovery-key-change'),
                padding: const EdgeInsets.all(Space.m),
                decoration: BoxDecoration(
                  color: palette.signal.withValues(alpha: 0.06),
                  border: Border.all(
                    color: palette.signal.withValues(alpha: 0.4),
                  ),
                  borderRadius: BorderRadius.circular(Radii.panel),
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      'Your account has a new recovery key',
                      style: type.body.copyWith(
                        color: palette.text,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    const SizedBox(height: Space.xs),
                    Text(
                      '${made(vault.recoveryKeyChangeMadeOn, vault.recoveryKeyChangeMadeMs.toInt())}. '
                      'Use it here only if you made it. Until you do, the key '
                      'you saved before still opens your secrets here.',
                      style: type.body.copyWith(color: palette.textMuted),
                    ),
                    const SizedBox(height: Space.m),
                    Button(
                      label: _busy ? 'Using…' : 'Use the key here',
                      onPressed: _busy ? null : _use,
                    ),
                    if (_error case final error?) ...[
                      const SizedBox(height: Space.xs),
                      Text(
                        error,
                        style: type.caption.copyWith(color: palette.danger),
                      ),
                    ],
                  ],
                ),
              ),
              const SizedBox(height: Space.l),
            ],
            if (vault.recoveryKeyCovered)
              SettingRow(
                label: 'Recovery key',
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      "Covered by your account's recovery key",
                      style: type.body.copyWith(color: palette.text),
                    ),
                    const SizedBox(height: Space.xs),
                    Text(
                      [
                        '${made(vault.recoveryKeyMadeOn, vault.recoveryKeyMadeMs.toInt())}.',
                        if (vault.recoveryKeyUsed)
                          'You used it here to open your secrets.',
                      ].join(' '),
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: type.caption.copyWith(color: palette.textMuted),
                    ),
                  ],
                ),
              ),
          ],
        );
      },
    );
  }
}
