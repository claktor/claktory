import 'package:flutter/widgets.dart';

import 'controls.dart' show ExtensionGlyph;
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';

/// Where what an agent asks or proposes may come from: the web pages it
/// read through browser control (a card's `taint`), in one line after a
/// small globe: "from Ada, which read a page on https://docs.example.com",
/// or "from Ada, which read 3 pages" (the sites in its semantics). A pipe
/// or direct messages read, on cards and records from older versions, show
/// nothing: reading them no longer means anything. Nothing when no page
/// was read. [agent] is the name of the agent whose card it is.
class TaintLabel extends StatelessWidget {
  const TaintLabel({super.key, required this.taint, required this.agent});

  final List<PipeTaint> taint;
  final String agent;

  @override
  Widget build(BuildContext context) {
    final line = taintLine(taint, agent: agent);
    if (line == null) return const SizedBox.shrink();
    final palette = Palette.of(context);
    final pages = taintPages(taint);
    return Padding(
      padding: const EdgeInsets.only(top: 2),
      child: Semantics(
        label: pages.length < 2
            ? line
            : '$line: ${[for (final page in pages) page.pipeName].join(', ')}',
        excludeSemantics: true,
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Padding(
              padding: const EdgeInsets.only(top: 2),
              child: ExtensionGlyph(
                icon: 'globe',
                name: '',
                color: palette.textMuted,
                size: 12,
              ),
            ),
            const SizedBox(width: Space.xs + 2),
            Expanded(
              child: Text(
                line,
                style: TextStyles.caption.copyWith(color: palette.textMuted),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// The web pages in [taint], each site once: what still counts.
List<PipeTaint> taintPages(List<PipeTaint> taint) {
  final seen = <String>{};
  return [
    for (final read in taint)
      if (read.kind == 'browser' && seen.add(read.pipeName)) read,
  ];
}

/// What [read] was, after "has read": a site read through browser control
/// ("a page on https://docs.example.com"); null for anything else, which
/// no longer counts.
String? taintRead(PipeTaint read) =>
    read.kind == 'browser' ? 'a page on ${read.pipeName}' : null;

/// [taint] in one line: "from Ada, which read a page on
/// https://docs.example.com", "from Ada, which read 3 pages"; null when it
/// read no page.
String? taintLine(List<PipeTaint> taint, {required String agent}) {
  final pages = taintPages(taint);
  if (pages.isEmpty) return null;
  final reader = pages.first.viaNames.isEmpty
      ? agent
      : pages.first.viaNames.first;
  return pages.length == 1
      ? 'from $reader, which read ${taintRead(pages.single)}'
      : 'from $reader, which read ${pages.length} pages';
}
