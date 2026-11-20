import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'extensions.dart' show dayLabel;
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'update.dart';
import 'selection.dart';

/// The version of Factory this window runs: the newer of the backend's and
/// this app's (an update that changed only the app leaves the backend at the
/// version before). An unstamped app ("dev") goes by the backend.
String installedVersion(Workspace workspace, String appVersion) {
  final backend = workspace.version;
  if (appVersion == 'dev' || appVersion.isEmpty) return backend;
  return isNewer(appVersion, backend) ? appVersion : backend;
}

/// The changelog's versions not installed yet: what the update brings.
List<ChangelogVersion> newVersions(Workspace workspace, String appVersion) {
  final installed = installedVersion(workspace, appVersion);
  return [
    for (final version in workspace.changelog)
      if (!version.unreleased && isNewer(version.version, installed)) version,
  ];
}

/// What changed in each version of Factory, newest first, as a timeline:
/// what an update would bring, marked new, then this version and the ones
/// before. Replaces the agent view until closed with Done or Esc.
class ChangelogView extends StatelessWidget {
  const ChangelogView({super.key});

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final scope = AppScope.scope(context);
    void close() => AppScope.closePage(context);
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): close},
      child: FocusScope(
        autofocus: true,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            WindowDrag(
              child: Container(
                height: titleBarHeight,
                padding: const EdgeInsets.only(left: Space.xl, right: Space.m),
                decoration: BoxDecoration(
                  border: Border(bottom: BorderSide(color: palette.hairline)),
                ),
                child: Row(
                  children: [
                    Text(
                      "What's new",
                      style: TextStyles.label.copyWith(
                        color: palette.text,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    const Spacer(),
                    Button(label: 'Done', onPressed: close),
                  ],
                ),
              ),
            ),
            Expanded(
              child: SelectableArea(
                child: ListenableBuilder(
                  listenable: scope.backend.workspace,
                  builder: (context, _) {
                    final workspace = scope.backend.workspace.workspace;
                    final installed = installedVersion(
                      workspace,
                      scope.appBuild.version,
                    );
                    final versions = workspace.changelog;
                    return Align(
                      alignment: Alignment.topCenter,
                      child: ConstrainedBox(
                        constraints: const BoxConstraints(maxWidth: 640),
                        child: versions.isEmpty
                            ? Padding(
                                padding: const EdgeInsets.all(Space.xl),
                                child: Text(
                                  'No release notes yet.',
                                  style: TextStyles.body.copyWith(
                                    color: palette.textFaint,
                                  ),
                                ),
                              )
                            : ListView.builder(
                                padding: const EdgeInsets.all(Space.xl),
                                itemCount: versions.length,
                                itemBuilder: (context, index) {
                                  final version = versions[index];
                                  return _Version(
                                    version: version,
                                    state: version.unreleased
                                        ? _State.unreleased
                                        : version.version == installed
                                        ? _State.installed
                                        : isNewer(version.version, installed)
                                        ? _State.offered
                                        : _State.earlier,
                                    last: index == versions.length - 1,
                                  );
                                },
                              ),
                      ),
                    );
                  },
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

enum _State { unreleased, offered, installed, earlier }

/// One version on the timeline: its dot on the line, its name, when it was
/// released and whether it's this one or still to install, and its items.
class _Version extends StatelessWidget {
  const _Version({
    required this.version,
    required this.state,
    required this.last,
  });

  final ChangelogVersion version;
  final _State state;

  /// Nothing comes after it, so the line ends at its dot.
  final bool last;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final released = version.released > 0
        ? dayLabel(
            DateTime.fromMillisecondsSinceEpoch(version.released.toInt()),
            DateTime.now(),
          )
        : '';
    final (String? tag, Color tagColor) = switch (state) {
      _State.unreleased => ('Not released yet', palette.textMuted),
      _State.offered => ('Update to get it', palette.signal),
      _State.installed => ('This version', palette.textMuted),
      _State.earlier => (null, palette.textFaint),
    };
    final dot = switch (state) {
      _State.offered => palette.signal,
      _State.installed => palette.text,
      _ => palette.textFaint,
    };
    return IntrinsicHeight(
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // The timeline: a dot per version, joined by a hairline.
          SizedBox(
            width: Space.l + 2,
            child: Column(
              children: [
                const SizedBox(height: 7),
                Dot(color: dot, size: 7),
                if (!last)
                  Expanded(
                    child: Container(
                      width: 1,
                      margin: const EdgeInsets.only(top: Space.xs),
                      color: palette.hairline,
                    ),
                  ),
              ],
            ),
          ),
          const SizedBox(width: Space.s),
          Expanded(
            child: Padding(
              padding: EdgeInsets.only(bottom: last ? 0 : Space.xl),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Wrap(
                    spacing: Space.s,
                    crossAxisAlignment: WrapCrossAlignment.center,
                    children: [
                      Text(
                        version.unreleased ? 'Unreleased' : version.version,
                        style: TextStyles.label.copyWith(
                          color: palette.text,
                          fontWeight: FontWeight.w600,
                          fontFeatures: const [FontFeature.tabularFigures()],
                        ),
                      ),
                      if (released.isNotEmpty)
                        Text(
                          released,
                          style: TextStyles.caption.copyWith(
                            color: palette.textFaint,
                          ),
                        ),
                      if (tag != null)
                        Text(
                          tag,
                          style: TextStyles.caption.copyWith(
                            color: tagColor,
                            fontWeight: FontWeight.w500,
                          ),
                        ),
                    ],
                  ),
                  const SizedBox(height: Space.s),
                  for (final item in version.items) _Item(item: item),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// What a kind of change is called, in its column; nothing for a line with
/// none of the prefixes.
String changeKindLabel(ChangelogKind kind) => switch (kind) {
  ChangelogKind.CHANGELOG_KIND_ADDED => 'Added',
  ChangelogKind.CHANGELOG_KIND_CHANGED => 'Changed',
  ChangelogKind.CHANGELOG_KIND_FIXED => 'Fixed',
  _ => '',
};

/// One change: its kind in a column of its own, so the texts line up, then
/// what changed.
class _Item extends StatelessWidget {
  const _Item({required this.item});

  final ChangelogItem item;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final kind = changeKindLabel(item.kind);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.xs + 2),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 60,
            child: Padding(
              // On the first line's baseline, near enough: caption is smaller.
              padding: const EdgeInsets.only(top: 2),
              child: Text(
                kind,
                style: TextStyles.caption.copyWith(color: palette.textFaint),
              ),
            ),
          ),
          Expanded(
            child: Text(
              item.text.isEmpty ? item.raw : item.text,
              style: TextStyles.body.copyWith(color: palette.text),
            ),
          ),
        ],
      ),
    );
  }
}
