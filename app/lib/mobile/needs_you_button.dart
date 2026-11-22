import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import '../inbox.dart' show needsYouChipLabel, needsYouCount;
import '../scope.dart';
import '../selection.dart' show Unselectable;
import '../waiting.dart' show NeedsYouMark;
import 'platform_policy.dart';

/// On a phone, how many things anywhere need the person, in a
/// conversation's header beside "…": the drawer's Needs you glyph with
/// their count on its corner, a 44pt button that opens them in a sheet
/// ([onOpen]). Where a Mac floats its chip over the conversation's top
/// right, a phone keeps the conversation clear (#321). Nothing while
/// nothing needs the person.
class PhoneNeedsYouButton extends StatelessWidget {
  const PhoneNeedsYouButton({super.key, required this.onOpen});

  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final count = needsYouCount(backend.workspace.workspace);
        if (count == 0) return const SizedBox.shrink();
        final palette = Palette.of(context);
        final target = PhonePlatformPolicy.of(context).touchTarget;
        return Unselectable(
          child: Semantics(
            container: true,
            button: true,
            label: needsYouChipLabel(count),
            onTap: onOpen,
            excludeSemantics: true,
            child: Pressable(
              onTap: onOpen,
              builder: (context, pressed) => SizedBox(
                width: target,
                height: target,
                child: Center(
                  child: AnimatedContainer(
                    duration: Motion.fast,
                    width: 36,
                    height: 36,
                    decoration: BoxDecoration(
                      color: pressed ? palette.hover : const Color(0x00000000),
                      borderRadius: BorderRadius.circular(Radii.panel),
                    ),
                    // A bar's glyph keeps its size whatever the text size;
                    // the count on it stays a badge.
                    child: MediaQuery.withNoTextScaling(
                      child: Stack(
                        clipBehavior: Clip.none,
                        alignment: Alignment.center,
                        children: [
                          ExtensionGlyph(
                            icon: 'inbox',
                            name: 'Needs you',
                            color: palette.text,
                            size: 20,
                          ),
                          Positioned(
                            top: 1,
                            right: -2,
                            child: NeedsYouMark(count: count, label: ''),
                          ),
                        ],
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}
