// A document surface with native ink (app/ios/Runner/Ink): a PDF with a
// canvas over each page (PaperView, iOS 16 and later), or one canvas over
// an image, an excerpt or a blank sketch (InkView). The workbench's
// document host embeds it as a pane leaf's body; it takes the leaf's
// constraints and follows them frame by frame. On a Mac the same surface,
// without ink, is the PDF viewer's page (viewers/pdf.dart).
//
// docs/ipad-ink-spike.md says why it's built as it is: it claims every
// touch that starts on it at once (else a drawer's swipe or a list's
// scroll takes the Pencil's stroke), and a leaf keeps its native view, ink
// and all, across column moves only under the same GlobalKey.

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'ink_bridge.dart';

/// The platform view's type, registered by Ink/InkChannel.swift.
const inkViewType = 'claktory/ink';

class InkSurface extends StatefulWidget {
  /// [key] must be the leaf's GlobalKey, the same wherever the leaf is
  /// shown, so moving it between columns keeps its native view.
  ///
  /// [params] are read once, when the native view is made; change the tool,
  /// the drawing policy, the page or whether it's active through
  /// [controller]. A different document is a different surface (a new key).
  const InkSurface({
    required Key super.key,
    required this.params,
    required this.controller,
  });

  final InkParams params;
  final InkSurfaceController controller;

  @override
  State<InkSurface> createState() => _InkSurfaceState();
}

class _InkSurfaceState extends State<InkSurface> {
  static final _claim = <Factory<OneSequenceGestureRecognizer>>{
    Factory<EagerGestureRecognizer>(EagerGestureRecognizer.new),
  };

  @override
  Widget build(BuildContext context) {
    final params = widget.params.toJson();
    void attach(int id) =>
        widget.controller.attach(MethodChannel('$inkViewType/$id'));
    // A Mac shows PDFs without ink, in PDFKit's own view
    // (app/macos/Runner/PaperViewer.swift, viewers/pdf.dart): the
    // trackpad, the pointer and the keyboard are AppKit's there.
    if (defaultTargetPlatform == TargetPlatform.macOS) {
      return AppKitView(
        viewType: inkViewType,
        creationParams: params,
        creationParamsCodec: const StandardMessageCodec(),
        onPlatformViewCreated: attach,
      );
    }
    // Only iOS and the Mac have the native side; the document host shows
    // documents without ink elsewhere and never builds this there.
    if (defaultTargetPlatform != TargetPlatform.iOS) {
      return const SizedBox.expand();
    }
    return UiKitView(
      viewType: inkViewType,
      creationParams: params,
      creationParamsCodec: const StandardMessageCodec(),
      // Only read (an iPhone's PDF page): nothing to protect from a
      // swipe, so the page's own gestures (Back from the edge) still work,
      // and PDFView scrolls and zooms with what no one else claims.
      gestureRecognizers: widget.params.drawingPolicy == InkDrawingPolicy.none
          ? const {}
          : _claim,
      onPlatformViewCreated: attach,
    );
  }
}
