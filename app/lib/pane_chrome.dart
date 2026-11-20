import 'package:flutter/widgets.dart';

/// Where a pane's header is, filled by what the pane shows: its details
/// and controls. Above each pane ([Pane] in panes.dart).
class PaneChrome extends InheritedWidget {
  const PaneChrome({
    super.key,
    required this.header,
    this.reloads,
    required super.child,
  });

  /// The pane's header with [details] and [controls]; null when there's no
  /// header.
  final Widget Function(String details, List<Widget> controls)? header;

  /// Fires when an agent shows the pane's file again: the viewer reads it
  /// again.
  final Listenable? reloads;

  static PaneChrome? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<PaneChrome>();

  @override
  bool updateShouldNotify(PaneChrome old) =>
      old.header != header || old.reloads != reloads;
}

/// A pane's content: its header (from [PaneChrome]), then [body].
class PaneScaffold extends StatelessWidget {
  const PaneScaffold({
    super.key,
    required this.body,
    this.details = '',
    this.controls = const [],
  });

  final Widget body;
  final String details;
  final List<Widget> controls;

  @override
  Widget build(BuildContext context) {
    final header = PaneChrome.of(context)?.header;
    if (header == null) return body;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        header(details, controls),
        Expanded(child: body),
      ],
    );
  }
}
