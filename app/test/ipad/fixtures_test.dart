// Pins the iPad document/ink fixtures (test/ipad/fixtures, README there):
// the ink bridge's messages between Dart and Swift, the capability report,
// and the evidence an annotation keeps. They are frozen: a change here
// needs Petra, Yuki and, for evidence, Omar.

import 'dart:convert';
import 'dart:io';
import 'dart:ui';

import 'package:factory_app/annotations.dart';
import 'package:flutter_test/flutter_test.dart';

final _dir = Directory('test/ipad/fixtures');

Map<String, Object?> _read(String path) =>
    jsonDecode(File('${_dir.path}/$path').readAsStringSync())
        as Map<String, Object?>;

List<String> _names(String sub) =>
    _dir
        .listSync()
        .whereType<Directory>()
        .where((d) => d.path.endsWith('/$sub'))
        .expand((d) => d.listSync())
        .whereType<File>()
        .map((f) => '$sub/${f.uri.pathSegments.last}')
        .where((n) => n.endsWith('.json'))
        .toList()
      ..sort();

final _sha = RegExp(r'^[0-9a-f]{64}$');

bool _atLeast(String version, String min) {
  List<int> parts(String v) => v.split('.').map(int.parse).toList();
  final a = parts(version), b = parts(min);
  for (var i = 0; i < 3; i++) {
    final x = i < a.length ? a[i] : 0, y = i < b.length ? b[i] : 0;
    if (x != y) return x > y;
  }
  return true;
}

/// Applies an affine transform [a, b, c, d, tx, ty] (x' = a·x + c·y + tx,
/// y' = b·x + d·y + ty) to a point.
Offset _apply(List<Object?> t, Offset p) {
  final m = t.cast<num>().map((n) => n.toDouble()).toList();
  return Offset(
    m[0] * p.dx + m[2] * p.dy + m[4],
    m[1] * p.dx + m[3] * p.dy + m[5],
  );
}

Rect _region(Object? json) {
  final r = json as Map<String, Object?>;
  double v(String k) => (r[k] as num).toDouble();
  return Rect.fromLTWH(v('x'), v('y'), v('w'), v('h'));
}

void main() {
  test('every fixture is JSON with schema_version 1', () {
    final all = [..._names('bridge'), ..._names('evidence')];
    expect(all, isNotEmpty);
    for (final name in all) {
      expect(_read(name)['schema_version'], 1, reason: name);
    }
  });

  group('capabilities', () {
    for (final name in _names(
      'bridge',
    ).where((n) => n.contains('capabilities_'))) {
      test(name, () {
        final c = _read(name);
        expect(c.keys.toSet(), {
          'schema_version',
          'idiom',
          'ios_app_on_mac',
          'os_version',
          'hardware_keyboard',
          'pencil',
          'pdf_page_overlay',
          'stroke_recognizer',
        });
        expect(['pad', 'phone', 'mac', 'unspecified'], contains(c['idiom']));
        expect(c['ios_app_on_mac'], isA<bool>());
        expect(c['hardware_keyboard'], isA<bool>());
        final pencil = c['pencil'] as Map;
        expect(pencil.keys.toSet(), {'hover', 'squeeze', 'double_tap'});
        // Pencil interactions are offered only on an iPad.
        if (c['idiom'] != 'pad') expect(pencil.values, everyElement(false));
        // Each gate says its minimum, so a change of minimum OS (Matt's
        // decision) changes values, never the shape.
        for (final gate in ['pdf_page_overlay', 'stroke_recognizer']) {
          final g = c[gate] as Map;
          expect(
            g['available'],
            _atLeast(c['os_version'] as String, g['min_os'] as String),
            reason: '$name $gate',
          );
        }
      });
    }

    test('keyboard change', () {
      final k = _read('bridge/keyboard_changed.json');
      expect(k['channel'], 'factory/device/keyboard');
      expect((k['event'] as Map).keys, ['hardware_keyboard']);
    });
  });

  group('surface creation', () {
    for (final name in _names('bridge').where((n) => n.contains('create_'))) {
      test(name, () {
        final f = _read(name);
        expect(f['view_type'], 'claktory/ink');
        final a = f['arguments'] as Map<String, Object?>;
        expect(a['leaf_id'], isA<String>());
        final surface = a['surface'];
        expect(['pdf', 'image', 'excerpt', 'blank'], contains(surface));
        expect([
          'default',
          'pencil_only',
          'any_input',
        ], contains(a['drawing_policy']));
        // Online-first or an on-device journal is Matt's decision to come:
        // a parameter, the same messages either way.
        expect([
          'online',
          'journal',
        ], contains((a['persistence'] as Map)['mode']));
        expect(a['active'], isA<bool>());
        final source = a['source'] as Map;
        final document = a['document'] as Map;
        switch (surface) {
          case 'pdf' || 'image':
            expect(source['sha256'], matches(_sha));
            // Native opens a non-authoritative cached copy and must check
            // it is the version the backend named.
            expect(document['sha256'], source['sha256']);
          case 'excerpt':
            final excerpt = document['excerpt'] as Map;
            expect(a['layers'], isEmpty);
            final quote = (excerpt['selectors'] as List).first as Map;
            expect(quote['type'], 'TextQuoteSelector');
            expect(excerpt['text'], quote['exact']);
          case 'blank':
            expect(document['blank'], isA<Map>());
        }
      });
    }
  });

  group('messages', () {
    const toNative = {
      'setTool',
      'showToolPicker',
      'undo',
      'redo',
      'loadDrawing',
      'setActive',
      'goToPage',
      'exportComposite',
    };
    const toDart = {
      'ready',
      'pageChanged',
      'selectionChanged',
      'drawingChanged',
      'undoState',
      'toolChanged',
      'strokeInterrupted',
      'error',
    };
    for (final name in _names(
      'bridge',
    ).where((n) => n.contains('/cmd_') || n.contains('/ev_'))) {
      test(name, () {
        final f = _read(name);
        expect(f['channel'], 'claktory/ink/<view_id>');
        if (name.contains('/cmd_')) {
          expect(f['direction'], 'dart_to_native');
          expect(toNative, contains(f['method']));
          expect(f.containsKey('result'), isTrue);
        } else {
          expect(f['direction'], 'native_to_dart');
          expect(toDart, contains(f['method']));
        }
      });
    }

    test('a drawing travels as PencilKit data with its hash', () {
      final d =
          (_read('bridge/ev_drawing_changed.json')['arguments']
                  as Map)['drawing']
              as Map;
      expect(d['format'], 'pencilkit');
      expect(d['content_version'], isA<int>());
      expect(d['sha256'], matches(_sha));
      expect(d['bytes_base64'], isA<String>());
    });

    test('a drawing change names its layer, or none yet', () {
      for (final name in _names(
        'bridge',
      ).where((n) => n.contains('ev_drawing_changed'))) {
        final a = _read(name)['arguments'] as Map;
        if (a['ink_layer_id'] == null) {
          // A page with no ink yet: Dart creates the layer.
          expect(a['base_revision'], isNull, reason: name);
        } else {
          expect(a['base_revision'], isA<int>(), reason: name);
        }
        final all = _region(a['bounds']), added = _region(a['new_bounds']);
        expect(all.expandToInclude(added), all, reason: name);
      }
    });

    test('errors name a known code', () {
      for (final name in _names(
        'bridge',
      ).where((n) => n.contains('ev_error'))) {
        expect(
          [
            'document_hash_mismatch',
            'document_unreadable',
            'document_locked',
            'os_too_old',
            'drawing_unreadable',
            'export_too_large',
          ],
          contains((_read(name)['arguments'] as Map)['code']),
          reason: name,
        );
      }
    });
  });

  group('evidence', () {
    final layers = {
      for (final name in _names('evidence').where((n) => n.contains('/layer_')))
        _read(name)['ink_layer_id'] as String: _read(name),
    };
    final items = _names('evidence').where((n) => n.contains('/item_'));

    test('layers: one drawing per page, image, excerpt or sketch', () {
      expect(layers, isNotEmpty);
      for (final layer in layers.values) {
        final id = layer['ink_layer_id'];
        expect(layer['revision'], isA<int>(), reason: '$id');
        final parent = layer['parent_revision'];
        if (parent != null) {
          expect(parent as int, lessThan(layer['revision'] as int));
        }
        expect((layer['source'] as Map)['sha256'], matches(_sha));
        final drawing = layer['drawing'] as Map;
        expect(drawing['format'], 'pencilkit');
        expect(drawing['sha256'], matches(_sha));
        expect(drawing['content_version'], isA<int>());
        final space = layer['coordinate_space'] as Map;
        if (space['kind'] == 'pdf_page') {
          expect(space['box'], 'crop');
          expect([0, 90, 180, 270], contains(space['rotation']));
          // A page overlay's drawing is in points from the CropBox's top
          // left, y down: its corners land on the CropBox's corners.
          expect(drawing['space'], 'page_overlay');
          final crop = (space['crop_box'] as List).cast<num>();
          final t = drawing['drawing_to_document'] as List;
          expect(
            _apply(t, Offset.zero),
            Offset(crop[0] * 1.0, (crop[1] + crop[3]) * 1.0),
          );
          expect(
            _apply(t, Offset(crop[2] * 1.0, crop[3] * 1.0)),
            Offset((crop[0] + crop[2]) * 1.0, crop[1] * 1.0),
          );
        }
      }
    });

    for (final name in items) {
      test(name, () {
        final e = _read(name);
        final layer = layers[e['ink_layer_id']];
        expect(layer, isNotNull, reason: 'its layer');
        expect(e['layer_revision'], layer!['revision']);
        expect(e['source_sha256'], (layer['source'] as Map)['sha256']);
        final composite = e['context_composite'] as Map;
        final long = [
          composite['pixel_width'] as int,
          composite['pixel_height'] as int,
        ].reduce((a, b) => a > b ? a : b);
        // What read_image passes through unchanged.
        expect(long, lessThanOrEqualTo(2048));
        expect(composite['bytes'] as int, lessThanOrEqualTo(3 * 1024 * 1024));

        final space = layer['coordinate_space'] as Map;
        final anchor = e['anchor'] as Map;
        final region = e['region'];
        if (region == null) {
          expect(anchor['kind'], anyOf('text', 'markdown'));
          final quote = (anchor['selectors'] as List).first as Map;
          expect(e['excerpt_text'], quote['exact']);
          expect(space['text'], quote['exact']);
          final position = (anchor['selectors'] as List)[1] as Map;
          // UTF-16 code units, as the backend's selectors count.
          expect(
            (position['end'] as int) - (position['start'] as int),
            (quote['exact'] as String).length,
          );
          return;
        }
        final rect = _region(region);
        expect(rect.left >= 0 && rect.top >= 0, isTrue);
        expect(rect.right <= 1 && rect.bottom <= 1, isTrue);
        final selector = (anchor['selectors'] as List).first as Map;
        switch (anchor['kind']) {
          case 'image':
            // The same region the image viewer's marks make today.
            expect(selector['value'], Region(rect).fragment);
          case 'pdf':
            expect(selector['value'], 'page=${space['page_1based']}');
            final refined = selector['refinedBy'] as Map;
            expect(refined['type'], 'ClaktoryPageRegion');
            expect(refined['box'], 'crop');
            expect(refined['rotation'], space['rotation']);
            expect(_region(refined), rect);
          default:
            fail('unknown kind ${anchor['kind']}');
        }
      });

      // The composite's transform takes its corners to the corners of its
      // region in the document: PDF points (unrotated, y up) for a page,
      // oriented pixels for an image, whatever the page's rotation.
      test('$name: composite transform', () {
        final e = _read(name);
        final space = layers[e['ink_layer_id']]!['coordinate_space'] as Map;
        _expectCompositeCovers(e['context_composite'] as Map, space);
      });
    }

    test('distinct blobs have distinct hashes', () {
      final seen = <String, String>{};
      void check(Map blob, String id) {
        final sha = blob['sha256'] as String;
        final key = '$id ${blob['pixel_width']}x${blob['pixel_height']}';
        expect(seen.putIfAbsent(sha, () => key), key, reason: sha);
      }

      for (final layer in layers.values) {
        final d = layer['drawing'] as Map;
        check(d, d['blob_id'] as String);
        final p = layer['ink_preview'] as Map;
        check(p, p['png_blob_id'] as String);
      }
      for (final name in items) {
        final c = _read(name)['context_composite'] as Map;
        check(c, c['png_blob_id'] as String);
      }
    });

    test('an outdated thread keeps its evidence on the old version', () {
      final o = _read('evidence/outdated.json');
      final thread = o['thread'] as Map;
      expect(thread['status'], 'OUTDATED');
      expect(thread['anchored_sha256'], isNot(thread['current_sha256']));
      expect((o['item'] as Map)['source_sha256'], thread['anchored_sha256']);
    });

    test('a detached thread keeps its excerpt', () {
      final d = _read('evidence/detached.json');
      final thread = d['thread'] as Map;
      expect(thread['status'], 'DETACHED');
      expect(thread['anchored_sha256'], isNot(thread['current_sha256']));
      expect((d['item'] as Map)['excerpt_text'], isA<String>());
    });

    test('a review batch sends chosen drafts once each', () {
      final b = _read('evidence/review_batch.json');
      // A retried Send can't deliver twice.
      expect(b['client_batch_id'], isA<String>());
      final batch = (b['items'] as List).cast<Map>();
      final threads = batch.map((i) => i['thread']).toList();
      expect(threads.toSet().length, threads.length);
      for (final excluded in b['excluded_drafts'] as List) {
        expect(threads, isNot(contains(excluded)));
      }
      for (final item in batch) {
        final layer = layers[item['ink_layer_id']];
        expect(layer, isNotNull);
        expect(item['layer_revision'], layer!['revision']);
        expect(
          item['page_1based'],
          (layer['coordinate_space'] as Map)['page_1based'],
        );
      }
    });

    test('the bridge and the evidence agree', () {
      final ready = _read('bridge/ev_ready.json')['arguments'] as Map;
      final pages = {
        for (final p in (ready['pages'] as List).cast<Map>())
          p['page_1based']: p,
      };
      for (final layer in layers.values) {
        final space = layer['coordinate_space'] as Map;
        if (space['kind'] != 'pdf_page') continue;
        final page = pages[space['page_1based']]!;
        expect(page['rotation'], space['rotation']);
        expect(page['crop_box'], space['crop_box']);
      }
      final created =
          (_read('bridge/create_pdf.json')['arguments'] as Map)['layers']
              as List;
      for (final l in created.cast<Map>()) {
        expect(l['revision'], layers[l['ink_layer_id']]!['revision']);
      }
      // Exporting t-41's context gives exactly the composite it keeps.
      final export = _read('bridge/cmd_export_composite.json');
      final result = export['result'] as Map;
      final kept =
          _read('evidence/item_pdf_region.json')['context_composite'] as Map;
      for (final key in [
        'pixel_width',
        'pixel_height',
        'bytes',
        'sha256',
        'region',
        'crop_to_document_transform',
      ]) {
        expect(result[key], kept[key], reason: key);
      }
      _expectCompositeCovers(
        result,
        layers['layer-paper-p3']!['coordinate_space'] as Map,
      );
    });
  });
}

void _expectCompositeCovers(Map composite, Map space) {
  final region = _region(composite['region']);
  final t = composite['crop_to_document_transform'] as List;
  final w = (composite['pixel_width'] as int).toDouble();
  final h = (composite['pixel_height'] as int).toDouble();
  final corners = [
    _apply(t, Offset.zero),
    _apply(t, Offset(w, 0)),
    _apply(t, Offset(0, h)),
    _apply(t, Offset(w, h)),
  ];
  final xs = corners.map((c) => c.dx), ys = corners.map((c) => c.dy);
  final got = Rect.fromLTRB(
    xs.reduce((a, b) => a < b ? a : b),
    ys.reduce((a, b) => a < b ? a : b),
    xs.reduce((a, b) => a > b ? a : b),
    ys.reduce((a, b) => a > b ? a : b),
  );
  late Rect want;
  switch (space['kind']) {
    case 'pdf_page':
      final crop = (space['crop_box'] as List).cast<num>();
      final cx = crop[0].toDouble(), cy = crop[1].toDouble();
      final cw = crop[2].toDouble(), ch = crop[3].toDouble();
      want = Rect.fromLTRB(
        cx + region.left * cw,
        cy + ch - region.bottom * ch,
        cx + region.right * cw,
        cy + ch - region.top * ch,
      );
    case 'image':
      final iw = (space['oriented_width'] as int).toDouble();
      final ih = (space['oriented_height'] as int).toDouble();
      want = Rect.fromLTRB(
        region.left * iw,
        region.top * ih,
        region.right * iw,
        region.bottom * ih,
      );
    default:
      final rw = (space['width'] as num).toDouble();
      final rh = (space['height'] as num).toDouble();
      want = Rect.fromLTRB(
        region.left * rw,
        region.top * rh,
        region.right * rw,
        region.bottom * rh,
      );
  }
  const tolerance = 1.0;
  for (final (a, b) in [
    (got.left, want.left),
    (got.top, want.top),
    (got.right, want.right),
    (got.bottom, want.bottom),
  ]) {
    expect((a - b).abs(), lessThan(tolerance), reason: '$got vs $want');
  }
}
