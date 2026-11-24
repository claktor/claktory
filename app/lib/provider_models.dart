// Where a provider's models come from (friend bug #14): Claktory reads each
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
/// the last read failed (the list there stays).
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
        if (error != null && !busy)
          Padding(
            padding: const EdgeInsets.only(top: Space.xs),
            child: ErrorLine(error, about: 'In Settings → Providers:'),
          ),
      ],
    );
  }
}
