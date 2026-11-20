import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'identity.dart';
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
  );

  const _Kind(this.title, this.about, this.empty);

  final String title;
  final String about;
  final String empty;
}

class _MarketplaceViewState extends State<MarketplaceView> {
  var _kind = _Kind.templates;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
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
                    // A phone has room for the tabs alone.
                    if (!isPhoneLayout(context)) ...[
                      Text(
                        'Marketplace',
                        style: TextStyles.label.copyWith(
                          color: palette.text,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                      const SizedBox(width: Space.xl),
                    ],
                    Expanded(
                      child: SingleChildScrollView(
                        scrollDirection: Axis.horizontal,
                        child: Row(
                          children: [
                            for (final kind in _Kind.values)
                              _Tab(
                                title: kind.title,
                                selected: kind == _kind,
                                onTap: () => setState(() => _kind = kind),
                              ),
                          ],
                        ),
                      ),
                    ),
                    Button(label: 'Done', onPressed: close),
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
                      padding: const EdgeInsets.all(Space.xl),
                      children: [
                        Text(
                          _kind.title,
                          style: TextStyles.emphasis.copyWith(
                            color: palette.text,
                          ),
                        ),
                        const SizedBox(height: Space.s),
                        Text(
                          _kind.about,
                          style: TextStyles.body.copyWith(
                            color: palette.textMuted,
                          ),
                        ),
                        const SizedBox(height: Space.xl),
                        ListenableBuilder(
                          listenable: AppScope.of(context).workspace,
                          builder: (context, _) {
                            final offered = _kind == _Kind.extensions
                                ? AppScope.of(context)
                                      .workspace
                                      .workspace
                                      .extensions
                                : const <Extension>[];
                            if (offered.isEmpty) {
                              return Text(
                                _kind.empty,
                                style: TextStyles.body.copyWith(
                                  color: palette.textFaint,
                                ),
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
                  style: TextStyles.label.copyWith(
                    color: palette.text,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(height: Space.xs),
                Text(
                  extension.description,
                  style: TextStyles.body.copyWith(color: palette.textMuted),
                ),
                const SizedBox(height: Space.s),
                Text(
                  extension.defaultEnabled
                      ? "On for every agent. Turn it off in an agent's settings."
                      : extension.adminDefault
                      ? 'On for Rivet. Turn it on for others in their settings.'
                      : "Turn it on in an agent's settings.",
                  style: TextStyles.caption.copyWith(color: palette.textFaint),
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
        height: 44,
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
          style: TextStyles.body.copyWith(
            color: selected || hovered ? palette.text : palette.textMuted,
          ),
        ),
      ),
    );
  }
}
