// Where a provider's models come from: Claktory reads each
// signed-in provider's list from the provider itself, every 12 hours and on
// Refresh models, so a new model is there without a new Claktory.
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'tell_rivet.dart' show ErrorLine;
import 'usage.dart' show formatAge;

/// What a provider's model list is now: being read, when it was last read
/// from the provider, or the list Claktory came with.
String modelsStatus(Provider provider, DateTime now) {
  if (provider.modelsFetching) return 'Refreshing models…';
  final fetched = provider.modelsFetchedMs.toInt();
  if (fetched > 0) return 'Models updated ${formatAge(fetched, now)}';
  return "Claktory's built-in list";
}

/// A provider's models in its card in Settings → Providers: [modelsStatus],
/// with Refresh models where the provider's list can be read now, and why
/// the last read failed (the list there stays). A read the provider refused
/// for its sign-in or key says "Sign-in expired" with Sign in again…, in
/// place of Tell Rivet: Rivet can't sign in for the person. It marks
/// nothing signed out: requests may still work.
class ProviderModels extends StatefulWidget {
  const ProviderModels({super.key, required this.provider});

  final Provider provider;

  @override
  State<ProviderModels> createState() => _ProviderModelsState();
}

class _ProviderModelsState extends State<ProviderModels> {
  /// Refresh models was pressed and hasn't been answered.
  var _asked = false;

  /// Why it was refused, when the provider doesn't say it itself.
  String? _failure;

  Future<void> _refresh() async {
    setState(() {
      _asked = true;
      _failure = null;
    });
    try {
      await AppScope.of(context).refreshModels(widget.provider.id);
    } on RequestFailure catch (error) {
      if (mounted) setState(() => _failure = error.message);
    } finally {
      if (mounted) setState(() => _asked = false);
    }
  }

  /// The usual sign-in, as an account's own Sign in again… starts it: with
  /// the first account's way, adding, so it replaces itself in place and
  /// never overwrites a working one.
  Future<void> _signInAgain() async {
    final provider = widget.provider;
    final first = provider.accounts.firstOrNull;
    final method = first?.methodId ?? provider.methods.firstOrNull?.id ?? '';
    setState(() => _failure = null);
    try {
      await AppScope.scope(context).login.start(
        provider.id,
        method.isEmpty ? 'oauth' : method,
        addAccount: first != null,
      );
    } on RequestFailure catch (error) {
      if (mounted) setState(() => _failure = error.message);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final provider = widget.provider;
    final busy = _asked || provider.modelsFetching;
    final error = provider.modelsFetchError.isNotEmpty
        ? provider.modelsFetchError
        : _failure;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Wrap(
          spacing: Space.m,
          runSpacing: Space.xs,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            Text(
              busy && !provider.modelsFetching
                  ? 'Refreshing models…'
                  : modelsStatus(provider, DateTime.now()),
              style: TypeScale.of(context).caption
                  .copyWith(color: palette.textMuted),
            ),
            if (provider.modelsFetchable)
              Button(
                label: busy ? 'Refreshing…' : 'Refresh models',
                onPressed: busy ? null : _refresh,
              ),
          ],
        ),
        if (provider.modelsFetchSignedOut &&
            provider.modelsFetchError.isNotEmpty &&
            !busy)
          Padding(
            padding: const EdgeInsets.only(top: Space.xs),
            child: Wrap(
              spacing: Space.s,
              runSpacing: Space.xs,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                Text(
                  'Sign-in expired',
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.danger),
                ),
                Button(label: 'Sign in again…', onPressed: _signInAgain),
                // Signing in couldn't start: that one is worth telling.
                if (_failure case final failure? when !_asked)
                  ErrorLine(failure, about: 'In Settings → Providers:'),
              ],
            ),
          )
        else if (error != null && !busy)
          Padding(
            padding: const EdgeInsets.only(top: Space.xs),
            child: ErrorLine(error, about: 'In Settings → Providers:'),
          ),
      ],
    );
  }
}
