import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'apps.dart' show AppsList;
import 'controls.dart';
import 'identity.dart';
import 'mobile/header.dart';
import 'mobile/metrics.dart';
import 'pane_chrome.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart';

/// What can be added to Factory, by kind. Replaces the agent view until
/// closed with Done or Esc.
class MarketplaceView extends StatefulWidget {
  const MarketplaceView({super.key});

  @override
  State<MarketplaceView> createState() => _MarketplaceViewState();
}

/// The marketplace's kinds, with what each is and what's offered.
enum _Kind {
  templates(
    'Agent templates',
    'Ready-made agents, each with its prompt, model and skills. Add one to '
        'your Claktory.',
    'No agent templates yet.',
  ),
  skills(
    'Skills',
    'Know-how for one kind of task: instructions and the scripts that go '
        'with them. Give them to any agent.',
    'No skills yet.',
  ),
  extensions(
    'Extensions',
    'New things your agents can do, added while Claktory runs.',
    'No extensions yet.',
  ),
  apps(
    'Apps',
    'Apps add things to Claktory, such as Charts, live charts of what your '
        "agents do. Tell Rivet what you'd like, and it sets an app up for "
        'you.',
    'No apps yet.',
  );

  const _Kind(this.title, this.about, this.empty);

  final String title;
  final String about;
  final String empty;
}

class _MarketplaceViewState extends State<MarketplaceView> {
  var _kind = _Kind.templates;
  ValueNotifier<String?>? _wanted;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final wanted = AppScope.of(context).marketplaceTab;
    if (wanted == _wanted) return;
    _wanted?.removeListener(_follow);
    _wanted = wanted..addListener(_follow);
    // Opened to show a tab: it's built with that one.
    _kind = _asked() ?? _kind;
  }

  @override
  void dispose() {
    _wanted?.removeListener(_follow);
    super.dispose();
  }

  /// The tab asked for ([Backend.marketplaceTab]), if any; it's cleared
  /// after this frame, so every Marketplace showing switches to it.
  _Kind? _asked() {
    final wanted = _wanted;
    final id = wanted?.value;
    if (wanted == null || id == null) return null;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (wanted.value == id) wanted.value = null;
    });
    return _kinds(context).where((kind) => kind.name == id).firstOrNull;
  }

  void _follow() {
    final kind = _asked();
    if (kind != null && kind != _kind && mounted) {
      setState(() => _kind = kind);
    }
  }

  /// The tabs there are: Apps only where the backend offers them.
  List<_Kind> _kinds(BuildContext context) => [
    for (final kind in _Kind.values)
      if (kind != _Kind.apps ||
          AppScope.of(context).workspace.workspace.appsAvailable)
        kind,
  ];

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    // Apps comes and goes with the backend's offer.
    listenable: AppScope.of(context).workspace,
    builder: (context, _) => _build(context),
  );

  Widget _build(BuildContext context) {
    final palette = Palette.of(context);
    final kinds = _kinds(context);
    final shown = kinds.contains(_kind) ? _kind : kinds.first;
    void close() => AppScope.closePage(context);
    // Beside a conversation, the pane's header names and closes it: the
    // bar keeps only the tabs.
    final inPane = InPane.of(context);
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): close},
      child: FocusScope(
        autofocus: !inPane,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (isPhoneLayout(context) && !inPane)
              // A phone's header, its tabs under the title.
              PhoneHeader(
                leading: HeaderButton.back(onTap: close),
                // One long word: smaller at the largest text sizes rather
                // than broken over two lines.
                title: FittedBox(
                  fit: BoxFit.scaleDown,
                  child: Text(
                    'Marketplace',
                    style: PhoneType.emphasis.copyWith(color: palette.text),
                  ),
                ),
                bottom: SingleChildScrollView(
                  scrollDirection: Axis.horizontal,
                  padding: const EdgeInsets.symmetric(horizontal: Space.s),
                  child: Row(
                    children: [
                      for (final kind in kinds)
                        _Tab(
                          title: kind.title,
                          selected: kind == shown,
                          onTap: () => setState(() => _kind = kind),
                        ),
                    ],
                  ),
                ),
              )
            else
              WindowDrag(
                child: Container(
                  height: titleBarHeight,
                  padding: const EdgeInsets.only(
                    left: Space.xl,
                    right: Space.m,
                  ),
                  decoration: BoxDecoration(
                    border: Border(bottom: BorderSide(color: palette.hairline)),
                  ),
                  child: Row(
                    children: [
                      Expanded(
                        // What Done leaves: the title in at most a third,
                        // cut short at the largest text in a narrow window,
                        // and the tabs in the rest.
                        child: LayoutBuilder(
                          builder: (context, constraints) => Row(
                            children: [
                              // A phone has room for the tabs alone.
                              if (!isPhoneLayout(context) && !inPane) ...[
                                ConstrainedBox(
                                  constraints: BoxConstraints(
                                    maxWidth: constraints.maxWidth / 3,
                                  ),
                                  child: Text(
                                    'Marketplace',
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                    style: TypeScale.of(context).label.copyWith(
                                      color: palette.text,
                                      fontWeight: FontWeight.w600,
                                    ),
                                  ),
                                ),
                                const SizedBox(width: Space.xl),
                              ],
                              Expanded(
                                child: SingleChildScrollView(
                                  scrollDirection: Axis.horizontal,
                                  child: Row(
                                    children: [
                                      for (final kind in kinds)
                                        _Tab(
                                          title: kind.title,
                                          selected: kind == shown,
                                          onTap: () =>
                                              setState(() => _kind = kind),
                                        ),
                                    ],
                                  ),
                                ),
                              ),
                            ],
                          ),
                        ),
                      ),
                      if (!inPane) Button(label: 'Done', onPressed: close),
                    ],
                  ),
                ),
              ),
            Expanded(
              child: Align(
                alignment: Alignment.topCenter,
                child: ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 640),
                  child: SelectableArea(
                    child: ListView(
                      padding: EdgeInsets.all(
                        isPhoneLayout(context) ? Touch.gutter : Space.xl,
                      ),
                      children: [
                        Text(
                          shown.title,
                          style: TypeScale.of(context).emphasis
                              .copyWith(color: palette.text),
                        ),
                        const SizedBox(height: Space.s),
                        Text(
                          shown.about,
                          style: TypeScale.of(context).body
                              .copyWith(color: palette.textMuted),
                        ),
                        const SizedBox(height: Space.xl),
                        if (shown == _Kind.apps)
                          const AppsList()
                        else
                          ListenableBuilder(
                            listenable: AppScope.of(context).workspace,
                            builder: (context, _) {
                              final offered = shown == _Kind.extensions
                                  ? AppScope.of(context)
                                        .workspace
                                        .workspace
                                        .extensions
                                  : const <Extension>[];
                              if (offered.isEmpty) {
                                return Text(
                                  shown.empty,
                                  style: TypeScale.of(context).body
                                      .copyWith(color: palette.textFaint),
                                );
                              }
                              return Column(
                                crossAxisAlignment: CrossAxisAlignment.stretch,
                                children: [
                                  for (final extension in offered)
                                    _ExtensionCard(extension: extension),
                                ],
                              );
                            },
                          ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// An extension Factory has: its icon, name and what it does, and whether
/// agents get it unless it's turned off for them.
class _ExtensionCard extends StatelessWidget {
  const _ExtensionCard({required this.extension});

  final Extension extension;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Container(
      margin: const EdgeInsets.only(bottom: Space.m),
      padding: const EdgeInsets.all(Space.l),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(top: 2),
            child: ExtensionGlyph(
              icon: extension.icon,
              name: extension.name,
              color: palette.text,
            ),
          ),
          const SizedBox(width: Space.m),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  extension.name,
                  style: TypeScale.of(context).label.copyWith(
                    color: palette.text,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(height: Space.xs),
                Text(
                  extension.description,
                  style: TypeScale.of(context).body
                      .copyWith(color: palette.textMuted),
                ),
                const SizedBox(height: Space.s),
                Text(
                  extension.defaultEnabled
                      ? "On for every agent. Turn it off in an agent's settings."
                      : extension.adminDefault
                      ? 'On for Rivet. Turn it on for others in their settings.'
                      : "Turn it on in an agent's settings.",
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.textFaint),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// A kind of thing in the header; the selected one is underlined.
class _Tab extends StatelessWidget {
  const _Tab({
    required this.title,
    required this.selected,
    required this.onTap,
  });

  final String title;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Pressable(
      onTap: onTap,
      builder: (context, hovered) => Container(
        // On a phone, taller as the text grows.
        height: isPhoneLayout(context) ? null : 44,
        constraints: isPhoneLayout(context)
            ? const BoxConstraints(minHeight: Touch.target)
            : null,
        padding: const EdgeInsets.symmetric(horizontal: Space.s),
        alignment: Alignment.center,
        decoration: BoxDecoration(
          border: Border(
            bottom: BorderSide(
              width: 2,
              color: selected ? palette.signal : const Color(0x00000000),
            ),
          ),
        ),
        child: Text(
          title,
          style: TypeScale.of(context).body.copyWith(
            color: selected || hovered ? palette.text : palette.textMuted,
          ),
        ),
      ),
    );
  }
}
