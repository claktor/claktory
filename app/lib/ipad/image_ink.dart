// Drawing on an image in an iPad pane (#362 I2): the image's own viewer,
// with the ink saved on it laid over it read-only (ink_preview.dart) and
// Draw, which turns the pane into the document host on an image surface,
// where Apple Pencil draws and what's drawn is kept as the image's ink
// layer; Done turns it back. Which panes are drawing is the window's
// DocumentSurfaces', in memory, by leaf.
import 'package:flutter/widgets.dart';

import '../ink_preview.dart' show InkDraw;
import '../protocol/factory/v1/factory.pb.dart' show ViewNode;
import '../scope.dart';
import 'document_host.dart' show DocumentHost;
import 'workbench.dart' show WorkbenchScope;

/// An image leaf's pane on an iPad: [viewer] under [InkDraw], or the
/// document host while it's drawn on. [agentId]: whose layout it's in, the
/// agent shown when null. With no workbench or no agent, just [viewer].
class ImageInkPane extends StatefulWidget {
  const ImageInkPane({
    super.key,
    required this.node,
    this.agentId,
    required this.viewer,
  });

  final ViewNode node;
  final String? agentId;
  final Widget Function(BuildContext context) viewer;

  @override
  State<ImageInkPane> createState() => _ImageInkPaneState();
}

class _ImageInkPaneState extends State<ImageInkPane> {
  String? _agent;

  void _draw() => _set(on: true);
  void _done() => _set(on: false);

  void _set({required bool on}) {
    final focus = WorkbenchScope.maybeOf(context);
    final agent = _agent;
    if (focus == null || agent == null) return;
    focus.documents.ink(agent, widget.node, on: on);
  }

  @override
  Widget build(BuildContext context) {
    final focus = WorkbenchScope.maybeOf(context);
    final agent = _agent =
        widget.agentId ?? AppScope.of(context).workspace.selectedAgent?.id;
    if (focus == null || agent == null || widget.node.leaf.path.isEmpty) {
      return widget.viewer(context);
    }
    final documents = focus.documents;
    return ValueListenableBuilder(
      valueListenable: documents.inking,
      builder: (context, _, _) => documents.inks(agent, widget.node)
          ? DocumentHost.forLeaf(
              context,
              widget.node,
              agentId: agent,
              onDone: _done,
            )
          : InkDraw(onDraw: _draw, child: widget.viewer(context)),
    );
  }
}
