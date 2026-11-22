// The ink bridge's Dart side: one `claktory/ink/<view id>` channel per
// document surface (ink_surface.dart), and the app-wide `claktory/ink`
// channel for the render cache. Its native side is app/ios/Runner/Ink.
//
// Pen motion never crosses it: PencilKit draws natively, and Dart hears of
// finished strokes (drawingChanged), coalesced, to save them to the backend
// as revisions of their ink layer. The shapes are the frozen fixtures in
// test/ipad/fixtures/bridge, pinned by test/ipad/ink_bridge_test.dart.

import 'dart:async';
import 'dart:convert';
import 'dart:developer' as developer;

import 'package:crypto/crypto.dart' as crypto;
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

Map<String, Object?> _map(Object? value) =>
    value is Map ? value.cast<String, Object?>() : const <String, Object?>{};

List<Map<String, Object?>> _maps(Object? value) =>
    value is List ? [for (final item in value) _map(item)] : const [];

double _double(Object? value, [double fallback = 0]) =>
    value is num ? value.toDouble() : fallback;

int? _int(Object? value) => value is num ? value.toInt() : null;

/// A region `{x, y, w, h}` in 0..1 from the top left, as a Rect.
Rect? inkRegion(Object? value) {
  if (value is! Map) return null;
  final m = _map(value);
  return Rect.fromLTWH(
    _double(m['x']),
    _double(m['y']),
    _double(m['w']),
    _double(m['h']),
  );
}

Map<String, Object?> inkRegionJson(Rect r) => {
  'x': _unit(r.left),
  'y': _unit(r.top),
  'w': _unit(r.width),
  'h': _unit(r.height),
};

/// A region's number to a billionth: a Rect keeps its right and bottom, so
/// a width read back has float dust (0.21 as 0.20999999999999996).
double _unit(double v) => (v * 1e9).round() / 1e9;

/// `#RRGGBBAA` (the fixtures') from an ARGB int (Flutter's).
String inkHex(int argb) {
  String byte(int shift) =>
      ((argb >> shift) & 0xFF).toRadixString(16).padLeft(2, '0').toUpperCase();
  return '#${byte(16)}${byte(8)}${byte(0)}${byte(24)}';
}

/// An ARGB int from `#RRGGBBAA` or `#RRGGBB`; null if it isn't one.
int? inkArgb(Object? hex) {
  if (hex is! String || !hex.startsWith('#')) return null;
  final digits = hex.substring(1);
  final value = int.tryParse(digits, radix: 16);
  if (value == null) return null;
  if (digits.length == 6) return 0xFF000000 | value;
  if (digits.length != 8) return null;
  return ((value & 0xFF) << 24) | (value >> 8);
}

// MARK: Creation

enum InkSurfaceKind { pdf, image, excerpt, blank }

enum InkDrawingPolicy {
  /// PencilKit's default: Pencil draws, fingers scroll, unless the person
  /// turned on "Only Draw with Apple Pencil" off in Settings.
  standard('default'),

  /// Only Apple Pencil draws; fingers scroll and zoom.
  pencilOnly('pencil_only'),

  /// Any touch draws (Draw with finger).
  anyInput('any_input'),

  /// Nothing draws: the document is only read (an iPhone's PDF page). It
  /// scrolls, zooms, selects and follows links.
  none('none');

  const InkDrawingPolicy(this.wire);
  final String wire;

  static InkDrawingPolicy parse(Object? wire) => values.firstWhere(
    (p) => p.wire == wire,
    orElse: () => InkDrawingPolicy.standard,
  );
}

/// The file on the backend a surface shows.
class InkSource {
  const InkSource({
    required this.backend,
    required this.path,
    this.sha256,
    this.mime,
    this.size,
  });

  final String backend;
  final String path;
  final String? sha256;
  final String? mime;
  final int? size;

  factory InkSource.fromJson(Map<String, Object?> json) => InkSource(
    backend: json['backend'] as String? ?? '',
    path: json['path'] as String? ?? '',
    sha256: json['sha256'] as String?,
    mime: json['mime'] as String?,
    size: _int(json['size']),
  );

  Map<String, Object?> toJson() => {
    'backend': backend,
    'path': path,
    'sha256': sha256,
    'mime': mime,
    'size': size,
  };
}

/// What native opens: a cached copy of the file (checked against its
/// hash), a frozen excerpt, or a blank canvas's size. Exactly one is set.
class InkDocument {
  const InkDocument.file({required String this.file, required this.sha256})
    : excerpt = null,
      blank = null;
  const InkDocument.excerpt(Map<String, Object?> this.excerpt)
    : file = null,
      sha256 = null,
      blank = null;
  const InkDocument.blank(Size this.blank)
    : file = null,
      sha256 = null,
      excerpt = null;

  final String? file;
  final String? sha256;

  /// `{text, selectors, rendition}`, as the evidence keeps it.
  final Map<String, Object?>? excerpt;
  final Size? blank;

  factory InkDocument.fromJson(Map<String, Object?> json) {
    if (json['excerpt'] is Map) {
      return InkDocument.excerpt(_map(json['excerpt']));
    }
    if (json['blank'] is Map) {
      final b = _map(json['blank']);
      return InkDocument.blank(Size(_double(b['width']), _double(b['height'])));
    }
    return InkDocument.file(
      file: json['file'] as String? ?? '',
      sha256: json['sha256'] as String?,
    );
  }

  Map<String, Object?> toJson() {
    if (excerpt != null) return {'excerpt': excerpt};
    if (blank != null) {
      return {
        'blank': {'width': _num(blank!.width), 'height': _num(blank!.height)},
      };
    }
    return {'file': file, 'sha256': sha256};
  }
}

/// Whole numbers stay ints on the wire, as the fixtures have them.
num _num(double value) =>
    value == value.roundToDouble() ? value.toInt() : value;

/// A page's ink layer and its revision (page null: the surface's only one).
class InkLayerRef {
  const InkLayerRef({
    this.page1based,
    required this.inkLayerId,
    required this.revision,
  });

  final int? page1based;

  /// Null for ink drawn here and not saved yet (a reload's answer).
  final String? inkLayerId;

  /// Null with [inkLayerId].
  final int? revision;

  factory InkLayerRef.fromJson(Map<String, Object?> json) => InkLayerRef(
    page1based: _int(json['page_1based']),
    inkLayerId: json['ink_layer_id'] as String?,
    revision: _int(json['revision']),
  );

  Map<String, Object?> toJson() => {
    'page_1based': page1based,
    'ink_layer_id': inkLayerId,
    'revision': revision,
  };
}

/// A surface's creation params (bridge fixtures create_*.json).
class InkParams {
  const InkParams({
    this.schemaVersion = 1,
    required this.leafId,
    required this.surface,
    required this.source,
    required this.document,
    this.layers = const [],
    this.drawingPolicy = InkDrawingPolicy.standard,
    this.persistence = 'online',
    this.active = true,
    this.paper = 0xFFFFFFFF,
    this.appearance = 'light',
    this.initialPage,
  });

  final int schemaVersion;

  /// The pane leaf the surface belongs to.
  final String leafId;
  final InkSurfaceKind surface;
  final InkSource source;
  final InkDocument document;
  final List<InkLayerRef> layers;
  final InkDrawingPolicy drawingPolicy;

  /// "online" or "journal".
  final String persistence;

  /// False while the column projection hides the leaf.
  final bool active;

  /// The paper's colour, ARGB.
  final int paper;

  /// "light" or "dark": how PencilKit shows ink colours.
  final String appearance;
  final int? initialPage;

  factory InkParams.fromJson(Map<String, Object?> json) => InkParams(
    schemaVersion: _int(json['schema_version']) ?? 1,
    leafId: json['leaf_id'] as String? ?? '',
    surface: InkSurfaceKind.values.byName(
      json['surface'] as String? ?? 'blank',
    ),
    source: InkSource.fromJson(_map(json['source'])),
    document: InkDocument.fromJson(_map(json['document'])),
    layers: [for (final l in _maps(json['layers'])) InkLayerRef.fromJson(l)],
    drawingPolicy: InkDrawingPolicy.parse(json['drawing_policy']),
    persistence: _map(json['persistence'])['mode'] as String? ?? 'online',
    active: json['active'] as bool? ?? true,
    paper: inkArgb(_map(json['ink'])['paper']) ?? 0xFFFFFFFF,
    appearance: _map(json['ink'])['appearance'] as String? ?? 'light',
    initialPage: _int(_map(json['initial'])['page_1based']),
  );

  Map<String, Object?> toJson() => {
    'schema_version': schemaVersion,
    'leaf_id': leafId,
    'surface': surface.name,
    'source': source.toJson(),
    'document': document.toJson(),
    'layers': [for (final l in layers) l.toJson()],
    'drawing_policy': drawingPolicy.wire,
    'persistence': {'mode': persistence},
    'active': active,
    'ink': {'paper': inkHex(paper), 'appearance': appearance},
    if (initialPage != null) 'initial': {'page_1based': initialPage},
  };
}

// MARK: Tools

enum InkToolKind { pen, pencil, marker, eraser, lasso }

/// A PencilKit tool. The workbench draws its own controls (PKToolPicker
/// takes focus from Flutter's text fields), so this is all native gets.
class InkTool {
  const InkTool({required this.kind, this.color, this.width = 3});

  final InkToolKind kind;

  /// ARGB; null for the eraser and the lasso.
  final int? color;
  final double width;

  factory InkTool.fromJson(Map<String, Object?> json) => InkTool(
    kind: InkToolKind.values.firstWhere(
      (k) => k.name == json['tool'],
      orElse: () => InkToolKind.pen,
    ),
    color: inkArgb(json['color']),
    width: _double(json['width'], 3),
  );

  Map<String, Object?> toJson() => {
    'tool': kind.name,
    'color': color == null ? null : inkHex(color!),
    'width': width,
  };

  @override
  bool operator ==(Object other) =>
      other is InkTool &&
      other.kind == kind &&
      other.color == color &&
      other.width == width;

  @override
  int get hashCode => Object.hash(kind, color, width);

  @override
  String toString() => 'InkTool(${kind.name}, $color, $width)';
}

// MARK: Drawings

/// A PencilKit drawing as it crosses the bridge and goes to the backend.
class InkDrawingData {
  const InkDrawingData({
    this.format = 'pencilkit',
    required this.contentVersion,
    required this.bytesBase64,
    required this.sha256,
  });

  InkDrawingData.bytes(
    Uint8List bytes, {
    required this.sha256,
    this.contentVersion = 1,
    this.format = 'pencilkit',
  }) : bytesBase64 = base64Encode(bytes);

  final String format;
  final int contentVersion;
  final String bytesBase64;
  final String sha256;

  Uint8List get bytes => base64Decode(bytesBase64);

  factory InkDrawingData.fromJson(Map<String, Object?> json) => InkDrawingData(
    format: json['format'] as String? ?? 'pencilkit',
    contentVersion: _int(json['content_version']) ?? 1,
    bytesBase64: json['bytes_base64'] as String? ?? '',
    sha256: json['sha256'] as String? ?? '',
  );

  Map<String, Object?> toJson() => {
    'format': format,
    'content_version': contentVersion,
    'bytes_base64': bytesBase64,
    'sha256': sha256,
  };
}

/// A saved layer's drawing, to load into the surface.
class InkLayerDrawing {
  const InkLayerDrawing({
    this.page1based,
    required this.inkLayerId,
    required this.revision,
    required InkDrawingData this.drawing,
    this.keepMine = false,
  });

  /// Nothing on [page1based] any more (its layer was deleted elsewhere):
  /// the page shows no ink, and has no layer.
  const InkLayerDrawing.cleared({this.page1based})
    : inkLayerId = null,
      revision = null,
      drawing = null,
      keepMine = false;

  final int? page1based;

  /// Null, with [revision], for ink drawn here and not saved yet (a view
  /// made again is loaded with it all the same).
  final String? inkLayerId;
  final int? revision;

  /// Null only for [InkLayerDrawing.cleared].
  final InkDrawingData? drawing;

  /// Keep both: [drawing] (another device's, saved first) goes under what's
  /// on the page now, and the two are sent at once as a drawingChanged
  /// (cause "keep_both") on this layer's [revision].
  final bool keepMine;

  Map<String, Object?> toJson() => {
    'page_1based': page1based,
    'ink_layer_id': inkLayerId,
    'revision': revision,
    'drawing': drawing?.toJson(),
    if (drawing == null) 'clear': true,
    if (keepMine) 'keep_mine': true,
  };
}

/// What loadDrawing did with each layer.
class InkLoadResult {
  const InkLoadResult({required this.loaded, required this.unreadable});

  /// `{page_1based, ink_layer_id, revision, strokes}` each.
  final List<Map<String, Object?>> loaded;

  /// `{page_1based, ink_layer_id, revision, content_version,
  /// max_content_version}` each: a newer PencilKit's, or damaged. Native
  /// also sends an `drawing_unreadable` error for each.
  final List<Map<String, Object?>> unreadable;

  factory InkLoadResult.fromJson(Map<String, Object?> json) => InkLoadResult(
    loaded: _maps(json['loaded']),
    unreadable: _maps(json['unreadable']),
  );
}

// MARK: Export

/// A composite of a region with its ink, for a review item's context.
class InkExportRequest {
  const InkExportRequest({
    this.page1based,
    this.inkLayerId,
    this.layerRevision,
    this.region,
    this.maxLongEdge = 2048,
    this.maxBytes = 3145728,
    this.includeInk = true,
  });

  final int? page1based;
  final String? inkLayerId;
  final int? layerRevision;

  /// Null for the whole page (its CropBox), a page's preview.
  final Rect? region;
  final int maxLongEdge;
  final int maxBytes;
  final bool includeInk;

  Map<String, Object?> toJson() => {
    'page_1based': page1based,
    'ink_layer_id': inkLayerId,
    'layer_revision': layerRevision,
    if (region != null) 'region': inkRegionJson(region!),
    'max_long_edge': maxLongEdge,
    'max_bytes': maxBytes,
    'include_ink': includeInk,
  };
}

/// The composite: a PNG in a temporary file, never through the channel.
class InkComposite {
  const InkComposite(this.raw);

  final Map<String, Object?> raw;

  String get file => raw['file'] as String? ?? '';
  String get mime => raw['mime'] as String? ?? 'image/png';
  int get pixelWidth => _int(raw['pixel_width']) ?? 0;
  int get pixelHeight => _int(raw['pixel_height']) ?? 0;
  int get bytes => _int(raw['bytes']) ?? 0;
  String get sha256 => raw['sha256'] as String? ?? '';
  Rect? get region => inkRegion(raw['region']);

  /// Composite pixels (y down) to PDF user space or oriented image pixels.
  List<double> get cropToDocumentTransform => [
    for (final v in raw['crop_to_document_transform'] as List? ?? const [])
      _double(v),
  ];
  String? get sourceSha256 => raw['source_sha256'] as String?;
  String? get drawingSha256 => raw['drawing_sha256'] as String?;
}

/// A new version of the PDF shown in place (bridge fixture
/// cmd_reload_document.json): its pages, where the person is now, the
/// pages that changed, came or went (compared page by page, never
/// remapped), the ink still shown (kept) and the ink of the earlier
/// version no longer shown (outdated), saved or not.
class InkReload {
  const InkReload(this.raw);

  final Map<String, Object?> raw;

  bool get reloaded => raw['reloaded'] as bool? ?? false;
  String get sourceSha256 => raw['source_sha256'] as String? ?? '';
  String? get previousSha256 => raw['previous_sha256'] as String?;
  int get pageCount => _int(raw['page_count']) ?? 0;
  List<InkPage> get pages => [
    for (final p in _maps(raw['pages'])) InkPage.fromJson(p),
  ];

  /// Where it's shown now.
  int? get page1based => _int(raw['page_1based']);
  List<int> get pagesChanged => _ints(raw['pages_changed']);
  List<int> get pagesAdded => _ints(raw['pages_added']);
  List<int> get pagesRemoved => _ints(raw['pages_removed']);

  /// How pages were compared ("content_then_render").
  String get comparison => raw['comparison'] as String? ?? '';
  List<InkLayerRef> get keptLayers => [
    for (final l in _maps(raw['kept_layers'])) InkLayerRef.fromJson(l),
  ];
  List<InkLayerRef> get outdatedLayers => [
    for (final l in _maps(raw['outdated_layers'])) InkLayerRef.fromJson(l),
  ];

  static List<int> _ints(Object? value) => [
    for (final v in value is List ? value : const []) ?_int(v),
  ];
}

/// A page's drawing as it is now, ready to save as its ink layer's next
/// revision (bridge fixture cmd_export_layer.json): the drawing and an
/// ink-only preview as temporary files for Dart to upload and delete, and
/// where they lie in the document, as `layer_*.json` keeps them.
class InkLayerExport {
  const InkLayerExport(this.raw);

  final Map<String, Object?> raw;

  Map<String, Object?> get _drawing => _map(raw['drawing']);
  Map<String, Object?> get _preview => _map(raw['ink_preview']);

  /// Null for a surface with one drawing (an image, an excerpt, a sketch).
  int? get page1based => _int(raw['page_1based']);

  /// Null before the layer's first save.
  String? get inkLayerId => raw['ink_layer_id'] as String?;

  /// The revision this one follows; null to create the layer.
  int? get baseRevision => _int(raw['base_revision']);

  /// The drawingChanged this export is; null when unchanged since loaded.
  int? get localSeq => _int(raw['local_seq']);
  String? get sourceSha256 => raw['source_sha256'] as String?;
  int get strokeCount => _int(raw['stroke_count']) ?? 0;

  /// The layer's `coordinate_space`, as the evidence fixtures have it.
  Map<String, Object?> get coordinateSpace => _map(raw['coordinate_space']);

  /// "page_overlay", "oriented_image", "excerpt_rendition" or
  /// "blank_canvas".
  String get drawingSpace => _drawing['space'] as String? ?? '';
  List<double> get drawingToDocument => [
    for (final v in _drawing['drawing_to_document'] as List? ?? const [])
      _double(v),
  ];
  int get contentVersion => _int(_drawing['content_version']) ?? 1;
  String get drawingFile => _drawing['file'] as String? ?? '';
  String get drawingSha256 => _drawing['sha256'] as String? ?? '';
  int get drawingBytes => _int(_drawing['bytes']) ?? 0;

  String get previewFile => _preview['file'] as String? ?? '';
  String get previewSha256 => _preview['sha256'] as String? ?? '';
  int get previewBytes => _int(_preview['bytes']) ?? 0;
  int get previewWidth => _int(_preview['pixel_width']) ?? 0;
  int get previewHeight => _int(_preview['pixel_height']) ?? 0;
}

// MARK: Search

/// A search's answer: its matches, or none when a newer search came first
/// ([superseded]).
class InkSearchResult {
  const InkSearchResult({
    required this.query,
    required this.matches,
    required this.truncated,
    required this.superseded,
  });

  final String query;
  final List<InkSearchMatch> matches;
  final bool truncated;
  final bool superseded;

  factory InkSearchResult.fromJson(Map<String, Object?> json) =>
      InkSearchResult(
        query: json['query'] as String? ?? '',
        matches: [
          for (final m in _maps(json['matches'])) InkSearchMatch.fromJson(m),
        ],
        truncated: json['truncated'] as bool? ?? false,
        superseded: json['superseded'] as bool? ?? false,
      );
}

/// One match: its page, where on it (0..1 from the CropBox's top left) and
/// its line.
class InkSearchMatch {
  const InkSearchMatch({
    required this.page1based,
    required this.region,
    required this.text,
  });

  final int page1based;
  final Rect? region;
  final String text;

  factory InkSearchMatch.fromJson(Map<String, Object?> json) => InkSearchMatch(
    page1based: _int(json['page_1based']) ?? 1,
    region: inkRegion(json['region']),
    text: json['text'] as String? ?? '',
  );
}

// MARK: Events

/// What a surface tells Dart (bridge fixtures ev_*.json).
sealed class InkEvent {
  const InkEvent();

  static InkEvent parse(String method, Object? arguments) {
    final a = _map(arguments);
    return switch (method) {
      'ready' => InkReady.fromJson(a),
      'pageChanged' => InkPageChanged(
        page1based: _int(a['page_1based']) ?? 1,
        pageCount: _int(a['page_count']) ?? 1,
      ),
      'drawingChanged' => InkDrawingChanged.fromJson(a),
      'selectionChanged' => InkSelectionChanged(
        page1based: _int(a['page_1based']),
        text: a['text'] as String? ?? '',
        region: inkRegion(a['region']),
      ),
      'undoState' => InkUndoState.fromJson(a),
      'toolChanged' => InkToolChanged(
        tool: InkTool.fromJson(a),
        source: a['source'] as String? ?? '',
      ),
      'strokeActive' => InkStrokeActive(
        page1based: _int(a['page_1based']),
        active: a['active'] as bool? ?? false,
      ),
      'strokeInterrupted' => InkStrokeInterrupted(
        page1based: _int(a['page_1based']),
        reason: a['reason'] as String? ?? '',
        kept: a['kept'] as bool? ?? false,
      ),
      'error' => InkError.fromJson(a),
      'linkTapped' => InkLinkTapped(a['url'] as String? ?? ''),
      'strokeSelectionChanged' => InkStrokeSelectionChanged(
        page1based: _int(a['page_1based']),
        strokeCount: _int(a['stroke_count']) ?? 0,
        bounds: inkRegion(a['bounds']),
      ),
      _ => InkUnknownEvent(method, a),
    };
  }
}

/// A page as native opened it. A PDF page has its boxes (`[x, y, w, h]` in
/// PDF user space) and /Rotate; an image, excerpt or sketch has its size.
class InkPage {
  const InkPage({
    this.page1based,
    this.cropBox,
    this.mediaBox,
    this.rotation = 0,
    this.size,
  });

  final int? page1based;
  final Rect? cropBox;
  final Rect? mediaBox;
  final int rotation;
  final Size? size;

  static Rect? _box(Object? value) {
    if (value is! List || value.length != 4) return null;
    return Rect.fromLTWH(
      _double(value[0]),
      _double(value[1]),
      _double(value[2]),
      _double(value[3]),
    );
  }

  factory InkPage.fromJson(Map<String, Object?> json) => InkPage(
    page1based: _int(json['page_1based']),
    cropBox: _box(json['crop_box']),
    mediaBox: _box(json['media_box']),
    rotation: _int(json['rotation']) ?? 0,
    size: json['width'] is num
        ? Size(_double(json['width']), _double(json['height']))
        : null,
  );
}

class InkReady extends InkEvent {
  const InkReady({
    required this.surface,
    this.sourceSha256,
    required this.sha256Verified,
    required this.pageCount,
    required this.pages,
    required this.drawingSpace,
    this.outline = const [],
    this.handwriting = InkHandwriting.unavailable,
  });

  final InkSurfaceKind surface;
  final String? sourceSha256;
  final bool sha256Verified;
  final int pageCount;
  final List<InkPage> pages;

  /// "page_overlay", "oriented_image", "excerpt_rendition" or "blank_canvas".
  final String drawingSpace;

  /// A PDF's own contents, when it has them (at most 500 entries).
  final List<InkOutlineEntry> outline;

  /// Whether this iPad reads handwriting (iPadOS 27 and later); an older
  /// native that doesn't say can't.
  final InkHandwriting handwriting;

  factory InkReady.fromJson(Map<String, Object?> json) => InkReady(
    surface: InkSurfaceKind.values.byName(
      json['surface'] as String? ?? 'blank',
    ),
    sourceSha256: json['source_sha256'] as String?,
    sha256Verified: json['sha256_verified'] as bool? ?? false,
    pageCount: _int(json['page_count']) ?? 1,
    pages: [for (final p in _maps(json['pages'])) InkPage.fromJson(p)],
    drawingSpace: json['drawing_space'] as String? ?? '',
    outline: [
      for (final entry in _maps(json['outline']))
        InkOutlineEntry.fromJson(entry),
    ],
    handwriting: json['handwriting'] is Map
        ? InkHandwriting.fromJson(_map(json['handwriting']))
        : InkHandwriting.unavailable,
  );
}

/// Reading handwriting on the device ([InkReady.handwriting]): whether it
/// can, from which iPadOS, and in which languages (BCP 47, as the system
/// gives them: "en", "hi-Latn").
class InkHandwriting {
  const InkHandwriting({
    required this.available,
    this.minOs = '',
    this.recognitionVersion,
    this.supportedLanguages = const [],
  });

  static const unavailable = InkHandwriting(available: false);

  final bool available;
  final String minOs;
  final int? recognitionVersion;
  final List<String> supportedLanguages;

  factory InkHandwriting.fromJson(Map<String, Object?> json) => InkHandwriting(
    available: json['available'] as bool? ?? false,
    minOs: json['min_os'] as String? ?? '',
    recognitionVersion: _int(json['recognition_version']),
    supportedLanguages: [
      for (final l in (json['supported_languages'] as List?) ?? const [])
        if (l is String) l,
    ],
  );
}

/// Which strokes [InkSurfaceController.recognizeText] reads.
enum InkRecognitionScope {
  /// Those chosen with the lasso on the page.
  selection,

  /// Those whose bounds meet a region (0..1 of the page).
  region,

  /// The page's whole layer.
  all,
}

/// What native read: a candidate, never applied or sent by native. No
/// [text] means nothing to read; [stale] means the drawing changed while
/// it read (drop it); not [available], an iPad below iPadOS 27.
class InkRecognition {
  const InkRecognition({
    required this.available,
    this.stale = false,
    this.page1based,
    this.inkLayerId,
    this.baseRevision,
    this.localSeq,
    this.drawingSha256,
    this.strokeCount = 0,
    this.text,
    this.languages = const [],
    this.recognitionVersion,
    this.reason,
    this.minOs,
  });

  final bool available;
  final bool stale;
  final int? page1based;
  final String? inkLayerId;
  final int? baseRevision;

  /// The drawingChanged it read (null: the page as loaded or saved).
  final int? localSeq;
  final String? drawingSha256;
  final int strokeCount;
  final String? text;
  final List<String> languages;
  final int? recognitionVersion;

  /// Not available: why ("os_too_old") and from which iPadOS.
  final String? reason;
  final String? minOs;

  /// Text worth offering: read, current and not blank.
  String? get candidate {
    final read = text?.trim();
    return available && !stale && read != null && read.isNotEmpty ? read : null;
  }

  factory InkRecognition.fromJson(Map<String, Object?> json) => InkRecognition(
    available: json['available'] as bool? ?? false,
    stale: json['stale'] as bool? ?? false,
    page1based: _int(json['page_1based']),
    inkLayerId: json['ink_layer_id'] as String?,
    baseRevision: _int(json['base_revision']),
    localSeq: _int(json['local_seq']),
    drawingSha256: json['drawing_sha256'] as String?,
    strokeCount: _int(json['stroke_count']) ?? 0,
    text: json['text'] as String?,
    languages: [
      for (final l in (json['languages'] as List?) ?? const [])
        if (l is String) l,
    ],
    recognitionVersion: _int(json['recognition_version']),
    reason: json['reason'] as String?,
    minOs: json['min_os'] as String?,
  );
}

/// An entry of a PDF's contents: its title, its page, how deep (from 0).
class InkOutlineEntry {
  const InkOutlineEntry({
    required this.title,
    required this.page1based,
    required this.depth,
  });

  final String title;
  final int page1based;
  final int depth;

  factory InkOutlineEntry.fromJson(Map<String, Object?> json) =>
      InkOutlineEntry(
        title: json['title'] as String? ?? '',
        page1based: _int(json['page_1based']) ?? 1,
        depth: _int(json['depth']) ?? 0,
      );
}

class InkPageChanged extends InkEvent {
  const InkPageChanged({required this.page1based, required this.pageCount});
  final int page1based;
  final int pageCount;
}

/// A finished stroke, erase, undo, redo or lasso move: the page's whole
/// drawing, to save as a new revision of its layer.
class InkDrawingChanged extends InkEvent {
  const InkDrawingChanged({
    this.page1based,
    this.inkLayerId,
    this.baseRevision,
    required this.localSeq,
    required this.cause,
    required this.strokeCount,
    this.bounds,
    this.newBounds,
    required this.drawing,
    this.sourceSha256,
  });

  final int? page1based;

  /// Null on a page with no ink yet: Dart creates the layer, then calls
  /// layerSaved.
  final String? inkLayerId;
  final int? baseRevision;
  final int localSeq;

  /// "stroke", "erase", "undo", "redo", "lasso" or "keep_both" (another
  /// device's drawing put under the person's).
  final String cause;
  final int strokeCount;

  /// The whole layer's ink, 0..1 from the top left.
  final Rect? bounds;

  /// The strokes added since the last event: a new item's default region.
  final Rect? newBounds;
  final InkDrawingData drawing;
  final String? sourceSha256;

  factory InkDrawingChanged.fromJson(Map<String, Object?> json) =>
      InkDrawingChanged(
        page1based: _int(json['page_1based']),
        inkLayerId: json['ink_layer_id'] as String?,
        baseRevision: _int(json['base_revision']),
        localSeq: _int(json['local_seq']) ?? 0,
        cause: json['cause'] as String? ?? 'stroke',
        strokeCount: _int(json['stroke_count']) ?? 0,
        bounds: inkRegion(json['bounds']),
        newBounds: inkRegion(json['new_bounds']),
        drawing: InkDrawingData.fromJson(_map(json['drawing'])),
        sourceSha256: json['source_sha256'] as String?,
      );
}

/// PDF text selected, for an item's quote.
class InkSelectionChanged extends InkEvent {
  const InkSelectionChanged({this.page1based, required this.text, this.region});
  final int? page1based;
  final String text;
  final Rect? region;
}

class InkUndoState extends InkEvent {
  const InkUndoState({required this.canUndo, required this.canRedo});
  final bool canUndo;
  final bool canRedo;

  factory InkUndoState.fromJson(Map<String, Object?> json) => InkUndoState(
    canUndo: json['can_undo'] as bool? ?? false,
    canRedo: json['can_redo'] as bool? ?? false,
  );
}

/// The tool changed natively (Apple Pencil's double-tap).
class InkToolChanged extends InkEvent {
  const InkToolChanged({required this.tool, required this.source});
  final InkTool tool;

  /// "pencil_double_tap".
  final String source;
}

/// A tool is down on the page (true) or lifted (false).
class InkStrokeActive extends InkEvent {
  const InkStrokeActive({this.page1based, required this.active});
  final int? page1based;
  final bool active;
}

/// A stroke cut short by a resize, rotation, undo or hiding. PencilKit
/// drops it (kept: false); strokeActive false comes first.
class InkStrokeInterrupted extends InkEvent {
  const InkStrokeInterrupted({
    this.page1based,
    required this.reason,
    required this.kept,
  });
  final int? page1based;
  final String reason;
  final bool kept;
}

/// The strokes chosen with the lasso on a page changed (iPadOS 27 and
/// later only): how many, and where (0..1 of the page). None chosen:
/// [strokeCount] 0 and no [bounds].
class InkStrokeSelectionChanged extends InkEvent {
  const InkStrokeSelectionChanged({
    this.page1based,
    required this.strokeCount,
    this.bounds,
  });
  final int? page1based;
  final int strokeCount;
  final Rect? bounds;
}

class InkError extends InkEvent {
  const InkError({
    required this.code,
    required this.recoverable,
    this.details = const {},
  });

  /// document_hash_mismatch, document_unreadable, document_locked,
  /// document_too_large, os_too_old, drawing_unreadable or
  /// export_too_large.
  final String code;
  final bool recoverable;

  /// The rest of the event's fields.
  final Map<String, Object?> details;

  /// document_too_large's: the PDF's pages and the most that open.
  int? get pageCount => _int(details['page_count']);
  int? get maxPages => _int(details['max_pages']);

  factory InkError.fromJson(Map<String, Object?> json) => InkError(
    code: json['code'] as String? ?? 'unknown',
    recoverable: json['recoverable'] as bool? ?? false,
    details: {
      for (final e in json.entries)
        if (e.key != 'code' && e.key != 'recoverable') e.key: e.value,
    },
  );
}

/// A link to outside the PDF was tapped: native opens nothing by itself;
/// the app offers to (http, https and mailto only).
class InkLinkTapped extends InkEvent {
  const InkLinkTapped(this.url);
  final String url;
}

/// An event this app doesn't know yet (a newer native side).
class InkUnknownEvent extends InkEvent {
  const InkUnknownEvent(this.method, this.arguments);
  final String method;
  final Map<String, Object?> arguments;
}

// MARK: Controller

/// Commands to one surface and its events. One per surface, kept for the
/// surface's life (with the leaf's GlobalKey, across column moves).
/// Commands sent before the native view exists wait for it.
class InkSurfaceController {
  InkSurfaceController() {
    _events = StreamController<InkEvent>.broadcast(onListen: _drain);
  }

  late final StreamController<InkEvent> _events;
  final _buffer = <InkEvent>[];
  final _attachedTo = Completer<MethodChannel>();
  MethodChannel? _channel;
  bool _disposed = false;

  /// Every event, in order. Those that came before the first listener are
  /// kept for it.
  Stream<InkEvent> get events => _events.stream;

  /// Whether the native view exists and hears commands.
  bool get attached => _channel != null;

  void _drain() {
    final waiting = List.of(_buffer);
    _buffer.clear();
    waiting.forEach(_events.add);
  }

  /// Joins the native view's channel and tells it to start sending events
  /// (it holds them until then). ink_surface.dart calls this. A surface
  /// made again (its leaf shown under another key) has a new view and a new
  /// channel: the controller follows it, and that view's `ready` says its
  /// drawings must be loaded again.
  Future<void> attach(MethodChannel channel) async {
    if (_disposed || _channel?.name == channel.name) return;
    _channel?.setMethodCallHandler(null);
    _channel = channel;
    channel.setMethodCallHandler((call) async {
      if (_disposed || _channel != channel) return null;
      final event = InkEvent.parse(call.method, call.arguments);
      if (_events.hasListener) {
        _events.add(event);
      } else {
        _buffer.add(event);
      }
      return null;
    });
    await channel.invokeMethod<void>('attach');
    if (!_attachedTo.isCompleted) _attachedTo.complete(channel);
  }

  Future<Map<String, Object?>> _call(
    String method, [
    Map<String, Object?>? arguments,
  ]) async {
    if (_disposed) throw StateError('InkSurfaceController was disposed');
    if (!_attachedTo.isCompleted) await _attachedTo.future;
    final channel = _channel!;
    final result = await channel.invokeMethod<Object?>(method, arguments);
    return _map(result);
  }

  /// The tool native now uses.
  Future<InkTool> setTool(InkTool tool) async =>
      InkTool.fromJson(await _call('setTool', tool.toJson()));

  /// Live: the same view, its ink untouched. Returns the policy in effect.
  Future<InkDrawingPolicy> setDrawingPolicy(InkDrawingPolicy policy) async =>
      InkDrawingPolicy.parse(
        (await _call('setDrawingPolicy', {
          'drawing_policy': policy.wire,
        }))['drawing_policy'],
      );

  Future<InkUndoState> undo() async =>
      InkUndoState.fromJson(await _call('undo'));

  Future<InkUndoState> redo() async =>
      InkUndoState.fromJson(await _call('redo'));

  /// Puts saved drawings on their pages. Not a change: no drawingChanged
  /// follows, and undo starts empty.
  Future<InkLoadResult> loadDrawing(List<InkLayerDrawing> layers) async =>
      InkLoadResult.fromJson(
        await _call('loadDrawing', {
          'layers': [for (final l in layers) l.toJson()],
        }),
      );

  /// Tells native a drawingChanged was saved: the layer's id (new on a page
  /// that had none) and its revision, the next change's base.
  Future<void> layerSaved({
    int? page1based,
    required String inkLayerId,
    required int revision,
  }) => _call('layerSaved', {
    'page_1based': page1based,
    'ink_layer_id': inkLayerId,
    'revision': revision,
  });

  /// False when the column projection hides the leaf: a stroke in progress
  /// is reported interrupted and pending changes are sent first. Returns
  /// whether they were.
  Future<bool> setActive(bool active) async =>
      (await _call('setActive', {'active': active}))['pending_flushed']
          as bool? ??
      false;

  /// PDF only, with [region] (0..1 of the page) scrolled into view when
  /// given: a search match, an item's anchor. Returns the page shown.
  Future<int> goToPage(int page1based, {Rect? region}) async =>
      _int(
        (await _call('goToPage', {
          'page_1based': page1based,
          if (region != null) 'region': inkRegionJson(region),
        }))['page_1based'],
      ) ??
      page1based;

  /// PDF only: finds [query]'s matches and highlights them on the pages;
  /// an empty query clears them. A newer search supersedes this one.
  Future<InkSearchResult> search(String query, {int maxResults = 200}) async =>
      InkSearchResult.fromJson(
        await _call('search', {'query': query, 'max_results': maxResults}),
      );

  /// Reads the handwriting of [scope] on [page1based] (null on an image or
  /// sketch); [region] with [InkRecognitionScope.region]. Below iPadOS 27
  /// an answer that isn't [InkRecognition.available], not an error.
  Future<InkRecognition> recognizeText(
    int? page1based, {
    required InkRecognitionScope scope,
    Rect? region,
    List<String>? languages,
  }) async => InkRecognition.fromJson(
    await _call('recognizeText', {
      'page_1based': page1based,
      'scope': scope.name,
      if (region != null) 'region': inkRegionJson(region),
      'languages': ?languages,
    }),
  );

  /// PDF only: shows [document], a new version of the file ([source]), in
  /// place. Throws a PlatformException `document_hash_mismatch`,
  /// `document_too_large`, `document_locked` or `document_unreadable`, and
  /// the version shown stays.
  Future<InkReload> reloadDocument(
    InkSource source,
    InkDocument document,
  ) async => InkReload(
    await _call('reloadDocument', {
      'source': source.toJson(),
      'document': document.toJson(),
    }),
  );

  Future<InkComposite> exportComposite(InkExportRequest request) async =>
      InkComposite(await _call('exportComposite', request.toJson()));

  /// [page1based]'s drawing (null: the surface's one drawing) as it is
  /// now, to save. Throws a PlatformException `nothing_drawn` for a page
  /// never drawn on.
  Future<InkLayerExport> exportLayer({
    int? page1based,
    int previewMaxLongEdge = 2048,
    int previewMaxBytes = 4 * 1024 * 1024,
  }) async => InkLayerExport(
    await _call('exportLayer', {
      'page_1based': page1based,
      'preview_max_long_edge': previewMaxLongEdge,
      'preview_max_bytes': previewMaxBytes,
    }),
  );

  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _channel?.setMethodCallHandler(null);
    if (!_attachedTo.isCompleted) {
      _attachedTo.future.ignore();
      _attachedTo.completeError(
        StateError('InkSurfaceController was disposed'),
      );
    }
    _buffer.clear();
    _events.close();
  }
}

// MARK: Render cache

const _appChannel = MethodChannel('claktory/ink');

/// The device's copies of backend files for native to open
/// (`Library/Caches/ink-render/<backend>/<sha256>/<name>`): a cache, never
/// the truth. A copy is kept only once its bytes match the hash.
class InkRenderCache {
  const InkRenderCache._();

  @visibleForTesting
  static MethodChannel channel = _appChannel;

  /// Starts writing a copy. Append its bytes in order, then finish.
  static Future<InkCacheWriter> begin({
    required String backend,
    required String sha256,
    required String name,
  }) async {
    final result = await channel.invokeMapMethod<String, Object?>(
      'cacheBegin',
      {'backend': backend, 'sha256': sha256, 'name': name},
    );
    return InkCacheWriter._(result?['token'] as String? ?? '');
  }

  /// The cached copy's path, or null if there's none.
  static Future<String?> lookup({
    required String backend,
    required String sha256,
    required String name,
  }) async {
    final result = await channel.invokeMapMethod<String, Object?>(
      'cacheLookup',
      {'backend': backend, 'sha256': sha256, 'name': name},
    );
    return result?['file'] as String?;
  }

  /// Removes one Claktory's copies, or every copy.
  static Future<void> clear({String? backend}) =>
      channel.invokeMethod<void>('cacheClear', {'backend': backend});
}

class InkCacheWriter {
  InkCacheWriter._(this.token);

  final String token;

  Future<void> append(Uint8List bytes) => InkRenderCache.channel
      .invokeMethod<void>('cacheAppend', {'token': token, 'bytes': bytes});

  /// The copy's path. Throws a PlatformException `hash_mismatch` if its
  /// bytes don't match, and keeps nothing.
  Future<String> finish() async {
    final result = await InkRenderCache.channel
        .invokeMapMethod<String, Object?>('cacheFinish', {'token': token});
    return result?['file'] as String? ?? '';
  }

  Future<void> cancel() => InkRenderCache.channel.invokeMethod<void>(
    'cacheCancel',
    {'token': token},
  );
}

// MARK: The journal of drawings not yet saved

/// A page's drawing whose latest change isn't saved yet, as the journal on
/// the iPad keeps it until its save is confirmed.
class InkJournalEntry {
  const InkJournalEntry({
    this.page1based,
    required this.seq,
    required this.sourceSha256,
    this.inkLayerId,
    this.baseRevision,
    required this.drawing,
  });

  final int? page1based;

  /// The change's local_seq.
  final int seq;

  /// The version of the file it was drawn on.
  final String sourceSha256;

  /// The layer it was to be the next revision of, and that layer's
  /// revision then; null for a page's first drawing.
  final String? inkLayerId;
  final int? baseRevision;
  final InkDrawingData drawing;

  Map<String, Object?> toJson() => {
    'page_1based': page1based,
    'seq': seq,
    'source_sha256': sourceSha256,
    'ink_layer_id': inkLayerId,
    'base_revision': baseRevision,
    'drawing': drawing.toJson(),
  };

  factory InkJournalEntry.fromJson(Map<String, Object?> json) =>
      InkJournalEntry(
        page1based: _int(json['page_1based']),
        seq: _int(json['seq']) ?? 0,
        sourceSha256: json['source_sha256'] as String? ?? '',
        inkLayerId: json['ink_layer_id'] as String?,
        baseRevision: _int(json['base_revision']),
        drawing: InkDrawingData.fromJson(
          (json['drawing'] as Map? ?? const {}).cast<String, Object?>(),
        ),
      );
}

/// Drawings not yet saved, kept on the iPad until the backend has them
/// (Matt's decision: the fourth thing a device keeps), by Claktory and
/// document, both named by hash. Native keeps them in the app's private
/// support folder, protected while the iPad is locked, never backed up.
class InkJournal {
  const InkJournal();

  /// A Claktory's name in the journal: its endpoint id (the tunnel's,
  /// stable for that Claktory), by hash; 'local' for one on this device.
  static String claktoryKey(String? endpointId) =>
      _hex(endpointId == null || endpointId.isEmpty ? 'local' : endpointId);

  /// A file's name in the journal, by hash.
  static String documentKey(String path) => _hex(path);

  static String _hex(String s) =>
      crypto.sha256.convert(utf8.encode(s)).toString();

  @visibleForTesting
  static MethodChannel channel = _appChannel;

  Future<void> put({
    required String claktory,
    required String document,
    required InkJournalEntry entry,
  }) => channel.invokeMethod<void>('journalPut', {
    'claktory': claktory,
    'document': document,
    ...entry.toJson(),
  });

  /// The page's entry, if its change is [uptoSeq] or older (any, without).
  Future<void> remove({
    required String claktory,
    required String document,
    int? page1based,
    int? uptoSeq,
  }) => channel.invokeMethod<void>('journalRemove', {
    'claktory': claktory,
    'document': document,
    'page_1based': page1based,
    'upto_seq': ?uptoSeq,
  });

  Future<List<InkJournalEntry>> list({
    required String claktory,
    required String document,
  }) async {
    final result = await channel.invokeMapMethod<String, Object?>(
      'journalList',
      {'claktory': claktory, 'document': document},
    );
    return [
      for (final e in _maps(result?['entries'])) InkJournalEntry.fromJson(e),
    ];
  }

  /// One Claktory's entries (a Claktory forgotten), or all (starting
  /// over). Never throws: where there's no journal (anything but iOS) it
  /// does nothing, and a failure is only logged, since forgetting goes on
  /// regardless.
  Future<void> clear({String? claktory}) async {
    try {
      await channel.invokeMethod<void>('journalClear', {'claktory': claktory});
    } on MissingPluginException {
      // No journal on this platform.
    } catch (e) {
      developer.log('ink journal: clear failed: $e', name: 'ink');
    }
  }
}
