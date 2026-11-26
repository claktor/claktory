// Settings → Storage (#446): how much of a Claktory's files this device
// keeps for its viewers, set on that Claktory for this device
// (Settings.file_cache), how much is kept now, and Clear. Only for a
// Claktory on another machine: this machine's own reads its files
// directly.
import 'dart:async';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';

import '../backend.dart';
import '../claktory_identity.dart' show claktoryName;
import '../controls.dart';
import '../identity.dart';
import '../mobile/metrics.dart' show TypeScale;
import '../resources.dart' show formatBytes;
import '../scope.dart';
import '../settings_view.dart' show SettingRequests, SettingRow;
import 'file_cache.dart';

/// "1 GB", "250 MB": a size Settings offers.
String fileCacheLabel(int bytes) {
  String trim(double value) => value == value.roundToDouble()
      ? '${value.round()}'
      : value.toStringAsFixed(1);
  return bytes >= 1000000000
      ? '${trim(bytes / 1e9)} GB'
      : '${trim(bytes / 1e6)} MB';
}

/// The size this device keeps of the Claktory shown, how much it keeps
/// now, and Clear.
class FileCacheSetting extends StatefulWidget {
  const FileCacheSetting({super.key});

  @override
  State<FileCacheSetting> createState() => _FileCacheSettingState();
}

class _FileCacheSettingState extends State<FileCacheSetting>
    with SettingRequests {
  Backend? _backend;
  FileCache? _cache;

  /// A size asked for and not yet the backend's.
  int? _pending;

  var _clearing = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final backend = AppScope.of(context);
    if (backend == _backend) return;
    _backend?.workspace.removeListener(_changed);
    _backend = backend..workspace.addListener(_changed);
    _changed();
  }

  @override
  void dispose() {
    _backend?.workspace.removeListener(_changed);
    super.dispose();
  }

  /// The backend's setting changed: the cache keeps to it, and what was
  /// asked for shows until it's there.
  void _changed() {
    final backend = _backend;
    if (backend == null) return;
    final cache = FileCache.of(backend);
    if (cache != _cache) {
      _cache = cache;
      // Counted now, for the line under it.
      if (cache != null) unawaited(cache.usage());
    }
    if (_pending == fileCacheLimit(backend)) _pending = null;
    if (mounted) setState(() {});
  }

  Future<void> _set(int bytes) async {
    final backend = _backend;
    if (backend == null) return;
    setState(() => _pending = bytes);
    await run(() => backend.setFileCache(bytes));
    if (mounted && failure != null) setState(() => _pending = null);
  }

  Future<void> _clear() async {
    final cache = _cache;
    if (cache == null) return;
    setState(() => _clearing = true);
    await cache.clear();
    if (mounted) setState(() => _clearing = false);
  }

  @override
  Widget build(BuildContext context) {
    final backend = _backend;
    final cache = _cache;
    if (backend == null || cache == null) return const SizedBox.shrink();
    final palette = Palette.of(context);
    final limit = _pending ?? fileCacheLimit(backend);
    final choices = {...fileCacheChoices, limit}.toList()..sort();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SettingRow(
          label: 'Saved files',
          top: true,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  SizedBox(
                    width: 120,
                    child: Select<int>(
                      title: 'Saved files',
                      value: limit,
                      options: [
                        for (final bytes in choices)
                          SelectOption(bytes, fileCacheLabel(bytes)),
                      ],
                      onChanged: (bytes) => unawaited(_set(bytes)),
                    ),
                  ),
                  const SizedBox(width: Space.s),
                  ValueListenableBuilder<int?>(
                    valueListenable: cache.used,
                    builder: (context, used, _) => Button(
                      label: 'Clear',
                      onPressed: _clearing || (used ?? 0) == 0
                          ? null
                          : () => unawaited(_clear()),
                    ),
                  ),
                ],
              ),
              const SizedBox(height: Space.s),
              ValueListenableBuilder<int?>(
                valueListenable: cache.used,
                builder: (context, used, _) => Text(
                  [
                    if (used != null) '${formatBytes(Int64(used))} used.',
                    'Pictures, videos, PDFs and text you open from '
                        '${claktoryName(backend)} are saved on this device, '
                        'so they open at once next time. When this is '
                        'full, the ones you opened longest ago go first.',
                  ].join(' '),
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.textMuted),
                ),
              ),
            ],
          ),
        ),
        failureLine(palette),
      ],
    );
  }
}
