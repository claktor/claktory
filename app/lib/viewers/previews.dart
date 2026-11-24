// When a viewer asks for a preview of its file rather than the file
// itself (#379). The backend makes it (an image fitted to this screen as
// WebP, a 720p video); the viewer says it's a preview and loads the
// original on request.
//
// - Images: on a phone or tablet always, since the screen can't show more
//   pixels than the preview has; on a Mac only on a slow link.
// - Videos: on any device, only on a slow link.
//
// A slow link is one #375's measured pace says would take over
// [slowerThan] for the original: its read stops after a piece or two once
// that's clear, and the preview is read instead.
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import '../backend.dart';
import '../paced.dart' show PreviewBox, ReadOnward;

/// Past this, the original is slow enough to come as a preview instead.
const slowerThan = Duration(seconds: 5);

/// Past this, an image can't come whole in a plain read (factoryd's
/// viewer.rs): it opens only as a preview.
const wholeImageLimit = 20000000;

/// Under this, nothing is worth a preview: the backend sends it as it is.
const previewFrom = 200 * 1000;

/// A phone or tablet, whose screen a preview is made for always.
bool get touchDevice =>
    defaultTargetPlatform == TargetPlatform.iOS ||
    defaultTargetPlatform == TargetPlatform.android;

/// The screen a preview is made for, in pixels, portrait (width the short
/// side): the device's largest display, whatever the window, so a pane
/// widened by a drag, or moved to a bigger display, isn't soft.
PreviewBox previewScreen() {
  final dispatcher = WidgetsBinding.instance.platformDispatcher;
  var short = 0.0, long = 0.0;
  for (final display in dispatcher.displays) {
    final size = display.size;
    if (size.longestSide > long) {
      long = size.longestSide;
      short = size.shortestSide;
    }
  }
  if (long == 0) {
    for (final view in dispatcher.views) {
      final size = view.physicalSize;
      if (size.longestSide > long) {
        long = size.longestSide;
        short = size.shortestSide;
      }
    }
  }
  // Nothing to measure (a test without a view): a big phone's.
  if (long == 0) return (width: 1320, height: 2868);
  return (width: short.round(), height: long.round());
}

/// Whether the [rest] of a file still to come would take longer than
/// [slowerThan] on this link, as its reads so far have measured it: false
/// while too little has been measured to say.
bool slowFor(Backend backend, int rest) {
  final takes = backend.readTime(rest);
  return takes != null && takes > slowerThan;
}

/// For a read of the original: stop once it's clear it would be slow, a
/// preview coming sooner (a file worth one, [previewFrom] and more).
ReadOnward stopWhenSlow(Backend backend) =>
    (through, size) => size < previewFrom || !slowFor(backend, size - through);
