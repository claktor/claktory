// Settings → Advanced diagnostics (#214, Secco's Workspace.diagnostics):
// whether this Claktory and its devices record how they work (connections
// first; other areas join as problems need them), off unless the person
// turns it on; while on, what each source kept, by area, with Delete for
// one source and Delete all. Turning it off deletes it all, after asking
// when anything is kept. Everything shown is the Workspace's; nothing is
// kept here.

import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'forms.dart' show FormFrame;
import 'identity.dart';
import 'mobile/metrics.dart';
import 'notification_settings.dart' show NotificationSwitch;
import 'protocol/factory/v1/factory.pb.dart';
import 'resources.dart' show formatBytes;
import 'scope.dart';
import 'settings_view.dart' show SettingRequests;

/// The switch's note: what's recorded, who reads it and how long it's
/// kept ([keepDays], left out when the backend doesn't say).
String diagnosticsNote(int keepDays) =>
    'Claktory and your devices record details of how they work, such as '
    'when a connection drops and why, so your agents can find out what goes '
    'wrong. Never your messages, codes or network addresses.'
    '${keepDays > 0 ? ' Kept on this Claktory for $keepDays days.' : ''}';

/// What the person calls [source]: this Claktory itself, a device by the
/// name it sent, else its paired name ([paired], by device id).
String diagnosticsSourceName(
  DiagnosticsSource source, {
  Map<String, String> paired = const {},
}) {
  if (source.id == 'backend') return 'This Claktory';
  if (source.name.trim().isNotEmpty) return source.name.trim();
  final device = source.id.startsWith('device:')
      ? source.id.substring('device:'.length)
      : source.id;
  if (paired[device] case final name? when name.isNotEmpty) return name;
  return device == 'local' ? 'This computer' : 'A device';
}

/// An area by its name: "connection" is Connections; one without a name
/// is General; a new one reads as its name does.
String diagnosticsAreaName(String area) => switch (area) {
  'connection' => 'Connections',
  '' || 'general' => 'General',
  _ => area[0].toUpperCase() + area.substring(1).replaceAll('_', ' '),
};

/// "84 KB over the last 3 days": [bytes] since [oldestMs] (0, unknown:
/// the size alone).
String diagnosticsAmount(Int64 bytes, Int64 oldestMs, DateTime now) {
  final size = formatBytes(bytes);
  if (oldestMs <= 0) return size;
  final age = now.difference(
    DateTime.fromMillisecondsSinceEpoch(oldestMs.toInt()),
  );
  String of(int n, String one, String many) => n <= 1 ? one : '$n $many';
  final span = age.inDays >= 1
      ? of(age.inDays, 'day', 'days')
      : age.inHours >= 1
      ? of(age.inHours, 'hour', 'hours')
      : of(age.inMinutes, 'minute', 'minutes');
  return '$size over the last $span';
}

/// A source's line: its name, its one area when it has only one, and how
/// much: "Studio iPad · Connections · 84 KB over the last 3 days".
String diagnosticsSourceLine(
  DiagnosticsSource source,
  DateTime now, {
  Map<String, String> paired = const {},
}) => [
  diagnosticsSourceName(source, paired: paired),
  if (source.areas.length == 1) diagnosticsAreaName(source.areas.single.area),
  diagnosticsAmount(source.bytes, source.oldestMs, now),
].join(' · ');

/// Advanced diagnostics' switch and, while it's on, what each source
/// kept. Only from a backend that records them.
class DiagnosticsSettings extends StatefulWidget {
  const DiagnosticsSettings({super.key, required this.workspace});

  final Workspace workspace;

  @override
  State<DiagnosticsSettings> createState() => _DiagnosticsSettingsState();
}

class _DiagnosticsSettingsState extends State<DiagnosticsSettings>
    with SettingRequests {
  void _send(SetDiagnostics ask) =>
      run(() => AppScope.of(context).setDiagnostics(ask));

  void _turn(bool on, Diagnostics diagnostics) {
    if (on || diagnostics.sources.isEmpty) {
      _send(SetDiagnostics(share: on));
      return;
    }
    _confirm(
      title: 'Turn off advanced diagnostics?',
      body: 'This deletes everything recorded so far.',
      submit: 'Turn off',
      then: () => _send(SetDiagnostics(share: false)),
    );
  }

  void _deleteAll() => _confirm(
    title: 'Delete all diagnostics?',
    body: null,
    submit: 'Delete',
    then: () => _send(SetDiagnostics(clear_2: true)),
  );

  void _confirm({
    required String title,
    required String? body,
    required String submit,
    required VoidCallback then,
  }) => showModal(
    context,
    (context, close) => FormFrame(
      title: title,
      body: body,
      fields: const [],
      error: null,
      submitLabel: submit,
      onSubmit: () {
        close();
        then();
      },
      onCancel: close,
    ),
  );

  @override
  Widget build(BuildContext context) {
    final workspace = widget.workspace;
    if (!workspace.hasDiagnostics()) return const SizedBox.shrink();
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final diagnostics = workspace.diagnostics;
    final now = DateTime.now();
    final paired = {
      for (final device in workspace.remote.devices) device.id: device.name,
    };
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        NotificationSwitch(
          label: 'Record advanced diagnostics',
          note: diagnosticsNote(diagnostics.keepDays),
          value: diagnostics.share,
          onChanged: (on) => _turn(on, diagnostics),
        ),
        if (diagnostics.share) ...[
          const SizedBox(height: Space.m),
          if (diagnostics.sources.isEmpty)
            Text(
              'Nothing received yet.',
              style: type.body.copyWith(color: palette.textFaint),
            )
          else ...[
            for (final source in diagnostics.sources)
              DiagnosticsSourceRow(
                key: ValueKey(source.id),
                line: diagnosticsSourceLine(source, now, paired: paired),
                areas: [
                  if (source.areas.length > 1)
                    for (final area in source.areas)
                      '${diagnosticsAreaName(area.area)} · '
                          '${formatBytes(area.bytes)}',
                ],
                onDelete: () =>
                    _send(SetDiagnostics(clear_2: true, sourceId: source.id)),
              ),
            Padding(
              padding: const EdgeInsets.only(top: Space.s),
              child: Align(
                alignment: Alignment.centerLeft,
                child: Button(label: 'Delete all', onPressed: _deleteAll),
              ),
            ),
          ],
        ],
        failureLine(palette),
      ],
    );
  }
}

/// One source's part: its line, its areas under it when it has several,
/// and Delete.
class DiagnosticsSourceRow extends StatelessWidget {
  const DiagnosticsSourceRow({
    super.key,
    required this.line,
    this.areas = const [],
    required this.onDelete,
  });

  final String line;
  final List<String> areas;
  final VoidCallback onDelete;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Container(
      constraints: BoxConstraints(
        minHeight: isPhoneLayout(context) ? Touch.target : 40,
      ),
      padding: const EdgeInsets.symmetric(vertical: Space.xs),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: palette.hairline)),
      ),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(line, style: type.body.copyWith(color: palette.text)),
                for (final area in areas)
                  Padding(
                    padding: const EdgeInsets.only(left: Space.l, top: 2),
                    child: Text(
                      area,
                      style: type.caption.copyWith(color: palette.textMuted),
                    ),
                  ),
              ],
            ),
          ),
          const SizedBox(width: Space.s),
          Button(label: 'Delete', onPressed: onDelete),
        ],
      ),
    );
  }
}
