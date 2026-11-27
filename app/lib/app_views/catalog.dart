import 'dart:convert';
import 'dart:math' as math;

import 'package:flutter/foundation.dart' show listEquals;
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import '../mobile/metrics.dart';
import '../move.dart' show MoveMeter;
import 'charts.dart';
import 'controller.dart';
import 'menu.dart';
import 'surface.dart';

/// Catalog v1 ("https://claktory.com/a2ui/catalog/v1"): the components an
/// app's view is made of (the charts, Chart, Stat and Table, in
/// charts.dart), drawn with the app's own widgets and IDENTITY's
/// look; an app never brings its own styling, code or web content. A
/// component this catalog doesn't know, or one not sent yet, takes no room.

/// What the person did with a component: its id, its action's event
/// (A2UI's `action.event`) and the data scope it was in.
typedef AppViewAct = void Function(
  String componentId,
  Map<String, Object?> event,
  String scope,
);

/// [surface] drawn from its root, following its changes.
class AppSurfaceView extends StatelessWidget {
  const AppSurfaceView({
    super.key,
    required this.surface,
    required this.onAction,
    required this.appName,
    this.enabled = true,
  });

  final AppSurfaceModel surface;
  final AppViewAct onAction;

  /// The app's name, on what it asks the person to type.
  final String appName;

  /// Off, it shows what it holds without taking input (a failed view's last
  /// good contents).
  final bool enabled;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: surface,
    builder: (context, _) => _Renderer(
      surface: surface,
      onAction: enabled ? onAction : null,
      appName: appName,
    ).node(context, 'root', '', 0),
  );
}

/// How deep components may nest; deeper ones take no room.
const _maxDepth = 32;

/// At most this many copies of a template are drawn; a caption says how
/// many more there are.
const _maxTemplateItems = 500;

/// Cards nested deeper than this lose their inset, so a deep view still
/// fits a phone's width.
const _maxInsetDepth = 8;

class _Renderer {
  _Renderer({
    required this.surface,
    required this.onAction,
    required this.appName,
  });

  final AppSurfaceModel surface;
  final AppViewAct? onAction;
  final String appName;

  /// How many TextFields each component holds, counted once per drawing.
  final _fieldCounts = <String, int>{};

  /// Whether what's being built sits in a horizontal List's line, where a
  /// Button takes only its own width (as it did when that line scrolled),
  /// never the whole line.
  var _inLine = false;

  /// Whether the next component built is a button on a bar (a Row of
  /// Buttons, one weighted: [_BarLayout]); only that one reads it.
  var _inBar = false;

  /// Whether what's being built is a bar button's face: one line, its
  /// words ending in "…" when there isn't room. Only what's built at once
  /// inside the face's _child reads it: a Text or List built later (in a
  /// LayoutBuilder or Builder) wouldn't see it.
  var _oneLine = false;

  /// How many TextFields component [id] holds, itself and in what it holds
  /// (a template's counts twice: it may repeat).
  int _fieldsIn(Object? id) {
    if (id is! String) return 0;
    final known = _fieldCounts[id];
    if (known != null) return known;
    // A component that holds itself adds nothing a second time.
    _fieldCounts[id] = 0;
    final c = surface.components[id];
    var count = 0;
    if (c != null) {
      if (c['component'] == 'TextField') count = 1;
      count += _fieldsIn(c['child']) + _fieldsInList(c['children']);
      final tabs = c['tabs'];
      if (tabs is List) {
        for (final tab in tabs) {
          if (tab is Map<String, Object?>) count += _fieldsIn(tab['child']);
        }
      }
    }
    return _fieldCounts[id] = count;
  }

  /// Whether component [id] says its fields' note itself, or something in
  /// it does: a Column or a vertical List holding more than one, or what
  /// holds all of them in one such.
  bool _notes(Object? id) {
    if (id is! String) return false;
    final known = _notesAt[id];
    if (known != null) return known;
    _notesAt[id] = false;
    final c = surface.components[id];
    final count = _fieldsIn(id);
    var notes = false;
    if (c != null && count > 1) {
      final kind = c['component'];
      if (kind == 'Column' ||
          (kind == 'List' && c['direction'] != 'horizontal')) {
        notes = true;
      } else {
        final list = c['children'];
        notes = [
          c['child'],
          if (list is List) ...list,
        ].any((child) => _fieldsIn(child) == count && _notes(child));
      }
    }
    return _notesAt[id] = notes;
  }

  /// [_notes], for each component, once per drawing.
  final _notesAt = <String, bool>{};

  /// [_fieldsIn], for a ChildList.
  int _fieldsInList(Object? list) {
    if (list is List) {
      return list.fold(0, (sum, id) => sum + _fieldsIn(id));
    }
    if (list is Map<String, Object?>) return 2 * _fieldsIn(list['componentId']);
    return 0;
  }

  /// A Column's or a vertical List's [list] of children, laid out by [lay].
  /// When they hold more than one field (a form), whose what's typed is
  /// is said once, above the first that holds one, rather than under each.
  Widget _noted(
    BuildContext context,
    Object? list,
    String scope,
    int depth,
    Widget Function(List<Widget> children) lay,
  ) {
    final built = _children(context, list, scope, depth);
    final count = _fieldsInList(list);
    final ids = list is List ? list.whereType<String>().toList() : const [];
    // Said nearest the fields: by the one child holding all of them, when
    // it says it.
    if (count < 2 || ids.any((id) => _fieldsIn(id) == count && _notes(id))) {
      return lay(built);
    }
    // Where the first field is: a template's are in its first copy.
    final at = math.max(0, ids.indexWhere((id) => _fieldsIn(id) > 0));
    return Builder(
      builder: (context) {
        // Said already, by a form this one is in.
        if (_FieldsNoted.on(context)) return lay(built);
        final palette = Palette.of(context);
        return _FieldsNoted(
          child: lay([
            ...built.take(at),
            Text(
              fieldsNote(appName),
              style: TypeScale.of(context).caption
                  .copyWith(color: palette.textFaint),
            ),
            ...built.skip(at),
          ]),
        );
      },
    );
  }

  Widget node(BuildContext context, String id, String scope, int depth) {
    final component = surface.components[id];
    if (component == null || depth > _maxDepth) return const SizedBox.shrink();
    final bar = _inBar;
    _inBar = false;
    final built = _component(context, id, component, scope, depth, bar: bar);
    return _accessible(component, scope, built);
  }

  Widget _component(
    BuildContext context,
    String id,
    Map<String, Object?> c,
    String scope,
    int depth, {
    bool bar = false,
  }) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    List<Widget> children(Object? list) =>
        _children(context, list, scope, depth);
    switch (c['component']) {
      case 'Text':
        final variant = c['variant'];
        final text = Text(
          string(c['text'], scope),
          maxLines: _oneLine ? 1 : (variant == 'title' ? 2 : null),
          softWrap: !_oneLine,
          overflow: _oneLine || variant == 'title'
              ? TextOverflow.ellipsis
              : null,
          style: switch (variant) {
            'heading' => type.emphasis.copyWith(
              color: palette.text,
              fontWeight: FontWeight.w600,
            ),
            'caption' => type.caption.copyWith(color: palette.textMuted),
            // A panel's title, as a Chart's label: Grafana's one style.
            'title' => panelTitle(context),
            _ => type.body.copyWith(color: palette.text),
          },
        );
        return variant == 'heading' || variant == 'title'
            ? Semantics(header: true, child: text)
            : text;
      case 'Row':
        final stackBelow = switch (c['stackBelow']) {
          final num n when n >= 320 && n <= 1200 => n.toDouble(),
          _ => null,
        };
        // A bar (a dashboard's top line): only Buttons, one weighted. On
        // one line whatever the width: the weighted one gives way first.
        if (c['children'] case final List ids
            when stackBelow == null &&
                ids.isNotEmpty &&
                ids.length <= 12 &&
                ids.every(
                  (id) =>
                      id is String &&
                      surface.components[id]?['component'] == 'Button',
                ) &&
                ids.any((id) => _weightOf(id as String) != null)) {
          final buttons = <Widget>[];
          for (final id in ids.cast<String>()) {
            _inBar = true;
            buttons.add(node(context, id, scope, depth + 1));
            _inBar = false;
          }
          return _BarLayout(
            weights: [for (final id in ids) _weightOf(id as String)],
            hasWords: [for (final id in ids) _hasWords(id as String)],
            labels: [for (final id in ids) _barLabel(id as String, scope)],
            children: buttons,
          );
        }
        final weighted = _weighted(context, c['children'], scope, depth);
        final anyWeight = weighted.any((one) => one.$2 != null);
        return LayoutBuilder(
          builder: (context, constraints) {
            // Weighted children (a dashboard's grid of stats) too narrow
            // side by side (a phone): one under another instead. Unweighted
            // ones (buttons, say) keep their line.
            final each = constraints.maxWidth / math.max(1, weighted.length);
            // The least grows with the text: at a large text size, half a
            // phone's width breaks words mid-word.
            final grow = MediaQuery.textScalerOf(context).scale(14) / 14;
            final least = minRowChildWidth * grow;
            final squeezed = anyWeight && weighted.length > 1 && each < least;
            // Narrower than the app asks (a dashboard on a phone, as
            // Grafana's under 768; more at a larger text size), or
            // squeezed: every child full width, one under another, without
            // the empty weighted Columns that fill a grid's gaps.
            if (stackBelow != null &&
                (constraints.maxWidth < stackBelow * grow || squeezed)) {
              // The ids in step with [weighted] (a template's have none).
              final ids = switch (c['children']) {
                final List list => [
                  for (final id in list)
                    if (id is String) id,
                ],
                _ => const <String>[],
              };
              // Panels 20 apart, as Grafana's are on a phone: within the
              // row, and from the next stacked row (a Column's 8 between,
              // and 6 either side).
              const stackedRowGap = 20.0;
              return Padding(
                padding: const EdgeInsets.symmetric(
                  vertical: stackedRowGap / 2 - Space.s / 2,
                ),
                child: Column(
                  spacing: stackedRowGap,
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    for (final (i, (child, _)) in weighted.indexed)
                      if (i >= ids.length ||
                          !_isGap(surface.components[ids[i]]))
                        child,
                  ],
                ),
              );
            }
            if (squeezed) {
              return Column(
                spacing: Space.s,
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [for (final (child, _) in weighted) child],
              );
            }
            // A dashboard's row of panels side by side: every panel as tall
            // as the tallest, as Grafana's grid lines up panels of one
            // height (a map is shorter than its place at a wide width, a
            // chart's legend makes it taller).
            if (stackBelow != null &&
                constraints.hasBoundedWidth &&
                weighted.isNotEmpty &&
                weighted.every((one) => one.$2 != null)) {
              return _EvenRow(
                weights: [for (final (_, weight) in weighted) weight!],
                spacing: Space.s,
                children: [for (final (child, _) in weighted) child],
              );
            }
            return Row(
              spacing: Space.s,
              mainAxisAlignment: _justify(c['justify']),
              crossAxisAlignment: _align(c['align'], CrossAxisAlignment.center),
              children: [
                for (final (child, weight) in weighted)
                  if (weight != null)
                    Expanded(flex: weight, child: child)
                  // Beside weighted ones, an unweighted child keeps its own
                  // width (up to its share) rather than a sliver of theirs.
                  else if (anyWeight && constraints.hasBoundedWidth)
                    ConstrainedBox(
                      constraints: BoxConstraints(maxWidth: each),
                      child: child,
                    )
                  else
                    Flexible(child: child),
              ],
            );
          },
        );
      case 'Column':
        final align = _align(c['align'], CrossAxisAlignment.stretch);
        // A panel's title (a "title" Text) sits as near what it titles as
        // a Chart's label does its plot, so a table's heading row is level
        // with the plot beside it.
        final ids = c['children'] is List ? c['children']! as List : const [];
        final titles = {
          for (final (i, id) in ids.indexed)
            if (i + 1 < ids.length &&
                id is String &&
                surface.components[id]?['component'] == 'Text' &&
                surface.components[id]?['variant'] == 'title')
              i,
        };
        return _noted(
          context,
          c['children'],
          scope,
          depth,
          (children) => Column(
            spacing: Space.s,
            mainAxisSize: MainAxisSize.min,
            mainAxisAlignment: _justify(c['justify']),
            crossAxisAlignment: align,
            children: titles.isEmpty || children.length != ids.length
                ? children
                : [
                    for (var i = 0; i < children.length; i++)
                      if (titles.contains(i))
                        Column(
                          spacing: Space.xs,
                          mainAxisSize: MainAxisSize.min,
                          crossAxisAlignment: align,
                          children: [children[i], children[i + 1]],
                        )
                      else if (!titles.contains(i - 1))
                        children[i],
                  ],
          ),
        );
      case 'List':
        if (c['direction'] == 'horizontal' && _oneLine) {
          // A bar button's face: one line, its words giving way.
          final ids = c['children'] is List ? c['children']! as List : const [];
          // Gaps as boxes: a Row's spacing isn't in its intrinsic width,
          // which the bar measures it by.
          return Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              for (final (i, child) in children(c['children']).indexed) ...[
                if (i > 0) const SizedBox(width: Space.s),
                if (i < ids.length &&
                    surface.components[ids[i]]?['component'] == 'Text')
                  Flexible(child: child)
                else
                  child,
              ],
            ],
          );
        }
        if (c['direction'] == 'horizontal') {
          // In a line while it fits; what doesn't fit goes on the next
          // line rather than off the screen (the largest text on a phone).
          final outer = _inLine;
          _inLine = true;
          final inLine = children(c['children']);
          _inLine = outer;
          return Wrap(
            spacing: Space.s,
            runSpacing: Space.xs,
            crossAxisAlignment: switch (c['align']) {
              'center' => WrapCrossAlignment.center,
              'end' => WrapCrossAlignment.end,
              _ => WrapCrossAlignment.start,
            },
            children: inLine,
          );
        }
        return _noted(
          context,
          c['children'],
          scope,
          depth,
          (children) => Column(
            spacing: Space.xs,
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: _align(c['align'], CrossAxisAlignment.stretch),
            children: children,
          ),
        );
      case 'Card':
        // A Stat reaches its card's edges, as a Grafana panel's does.
        final child = c['child'];
        if (child is String &&
            surface.components[child]?['component'] == 'Stat' &&
            depth <= _maxInsetDepth) {
          return AppStatCard(child: _child(context, child, scope, depth));
        }
        return Container(
          padding: EdgeInsets.all(depth > _maxInsetDepth ? 0 : Space.m),
          decoration: BoxDecoration(
            color: palette.sidebar,
            border: Border.all(color: palette.hairline),
            borderRadius: BorderRadius.circular(Radii.panel),
          ),
          child: _child(context, c['child'], scope, depth),
        );
      case 'Divider':
        return c['axis'] == 'vertical'
            ? SizedBox(
                width: 1,
                height: Space.l,
                child: ColoredBox(color: palette.hairline),
              )
            : SizedBox(height: 1, child: ColoredBox(color: palette.hairline));
      case 'Icon':
        return AppViewIcon(name: string(c['name'], scope));
      case 'Button':
        return _button(context, id, c, scope, depth, bar: bar);
      case 'TextField':
        return _BoundTextField(
          key: ValueKey('$id@$scope'),
          surface: surface,
          value: c['value'],
          scope: scope,
          label: string(c['label'], scope),
          placeholder: string(c['placeholder'], scope),
          variant: c['variant'] is String ? c['variant']! as String : '',
          enabled: onAction != null,
          appName: appName,
        );
      case 'CheckBox':
        final value = c['value'];
        return Row(
          spacing: Space.m,
          children: [
            Expanded(
              child: Text(
                string(c['label'], scope),
                style: type.body.copyWith(color: palette.text),
              ),
            ),
            Toggle(
              label: string(c['label'], scope),
              value: boolean(value, scope),
              onChanged: onAction == null || _pointer(value, scope) == null
                  ? null
                  : (on) => surface.write(_pointer(value, scope)!, on),
            ),
          ],
        );
      case 'ChoicePicker':
        return _choicePicker(context, c, scope);
      case 'Slider':
        return _BoundSlider(
          surface: surface,
          value: c['value'],
          scope: scope,
          label: string(c['label'], scope),
          min: number(c['min'], scope) ?? 0,
          max: number(c['max'], scope) ?? 1,
          enabled: onAction != null,
        );
      case 'Tabs':
        final tabs = c['tabs'];
        return _Tabs(
          key: ValueKey('$id@$scope'),
          titles: [
            if (tabs is List)
              for (final tab in tabs)
                if (tab is Map<String, Object?>) string(tab['title'], scope),
          ],
          child: (index) {
            final tab = (tabs! as List)[index];
            return tab is Map<String, Object?>
                ? _child(context, tab['child'], scope, depth)
                : const SizedBox.shrink();
          },
        );
      case 'ProgressBar':
        final value = (number(c['value'], scope) ?? 0).clamp(0.0, 1.0);
        final label = string(c['label'], scope);
        return Semantics(
          label: label.isEmpty ? null : label,
          value: '${(value * 100).round()}%',
          child: ExcludeSemantics(
            child: Column(
              spacing: Space.xs,
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (label.isNotEmpty)
                  Text(
                    label,
                    style: type.caption.copyWith(color: palette.textMuted),
                  ),
                MoveMeter(value: value.toDouble()),
              ],
            ),
          ),
        );
      case 'Chart':
        final height = number(c['height'], scope) ?? 200;
        double? instant(Object? value) => switch (number(value, scope)) {
          final n? when n >= 0 && n <= 8.64e15 && n == n.roundToDouble() =>
            n.toDouble(),
          _ => null,
        };
        var from = instant(c['timeFrom']);
        var to = instant(c['timeTo']);
        if (from != null && to != null && from >= to) from = to = null;
        return AppChart(
          key: ValueKey('$id@$scope'),
          kind: c['kind'] is String ? c['kind']! as String : '',
          table: AppTable.from(resolveValue(surface, c['data'], scope)),
          x: c['x'] is String ? c['x']! as String : null,
          y: [
            if (c['y'] case final List y)
              for (final name in y)
                if (name is String) name,
          ],
          seriesBy: c['seriesBy'] is String ? c['seriesBy']! as String : null,
          color: c['color'] is String ? c['color']! as String : 'palette',
          label: string(c['label'], scope),
          unit: string(c['unit'], scope),
          height: height.clamp(120, 480).toDouble(),
          // Grafana's per-panel settings: each read only when it's the
          // right kind of value in range, else the renderer's default.
          lineWidth: switch (c['lineWidth']) {
            final num n when n >= 0 && n <= 10 => n.toDouble(),
            _ => 1,
          },
          fill: switch (c['fill']) {
            final num n when n >= 0 && n <= 100 => n.toDouble(),
            _ => 0,
          },
          points: switch (c['points']) {
            final String p when const {'auto', 'always', 'never'}.contains(p) =>
              p,
            _ => 'auto',
          },
          pointSize: switch (c['pointSize']) {
            final num n when n >= 1 && n <= 40 => n.toDouble(),
            _ => 5,
          },
          min: switch (c['min']) {
            final num n when n.isFinite => n.toDouble(),
            _ => null,
          },
          max: switch (c['max']) {
            final num n when n.isFinite => n.toDouble(),
            _ => null,
          },
          decimals: switch (c['decimals']) {
            final num n when n >= 0 && n <= 10 && n == n.roundToDouble() =>
              n.toInt(),
            _ => null,
          },
          legend: c['legend'] != false,
          // The dashboard's time range, in milliseconds since 1970: whole,
          // not negative, and in order when both are set.
          timeFrom: from,
          timeTo: to,
        );
      case 'Map':
        final color = c['color'];
        return AppMap(
          table: AppTable.from(resolveValue(surface, c['data'], scope)),
          location: c['location'] is String ? c['location']! as String : null,
          value: c['value'] is String ? c['value']! as String : null,
          label: string(c['label'], scope),
          unit: string(c['unit'], scope),
          decimals: switch (c['decimals']) {
            final num n when n >= 0 && n <= 10 && n == n.roundToDouble() =>
              n.toInt(),
            _ => null,
          },
          thresholds: _thresholds(c['thresholds']),
          color: color is String ? color : null,
          sizeMin: switch (c['sizeMin']) {
            final num n when n >= 1 && n <= 40 => n.toDouble(),
            _ => 2,
          },
          sizeMax: switch (c['sizeMax']) {
            final num n when n >= 1 && n <= 40 => n.toDouble(),
            _ => 15,
          },
          legend: c['legend'] != false,
          height: switch (c['height']) {
            final num n when n >= 120 && n <= 480 => n.toDouble(),
            _ => null,
          },
        );
      case 'Stat':
        final color = c['color'];
        return AppStat(
          label: string(c['label'], scope),
          value: resolveValue(surface, c['value'], scope),
          unit: string(c['unit'], scope),
          spark: [
            if (resolveValue(surface, c['spark'], scope) case final List spark)
              for (final point in spark) ?cellNumber(point),
          ],
          color: color is String ? color : null,
          // Grafana's settings: each read only when it's the right kind of
          // value, else the renderer's default.
          thresholds: _thresholds(c['thresholds']),
          colorMode: switch (c['colorMode']) {
            final String m
                when const {
                  'value',
                  'background',
                  'solid',
                  'none',
                }.contains(m) =>
              m,
            _ => 'value',
          },
          graph: c['graph'] == 'none' ? 'none' : 'area',
          decimals: switch (c['decimals']) {
            final num n when n >= 0 && n <= 10 && n == n.roundToDouble() =>
              n.toInt(),
            _ => null,
          },
          height: switch (c['height']) {
            final num n when n >= 60 && n <= 480 => n.toDouble(),
            _ => null,
          },
        );
      case 'Table':
        final table = AppTable.from(resolveValue(surface, c['data'], scope));
        final columns = c['columns'];
        return AppTableView(
          table: table,
          height: switch (c['height']) {
            final num n when n >= 120 && n <= 480 => n.toDouble(),
            _ => null,
          },
          columns: table == null || columns is! List
              ? null
              : [
                  for (final column in columns)
                    if (column is Map<String, Object?> &&
                        column['name'] is String &&
                        table.columns.contains(column['name']))
                      (
                        index: table.columns.indexOf(column['name']! as String),
                        title: column.containsKey('title')
                            ? string(column['title'], scope)
                            : column['name']! as String,
                        format: switch (column['format']) {
                          'number' => 'number',
                          'time' => 'time',
                          _ => 'text',
                        },
                      ),
                ],
        );
      case 'EmptyState':
        final icon = c['icon'];
        final body = string(c['body'], scope);
        return Padding(
          padding: const EdgeInsets.symmetric(vertical: Space.xl),
          child: Column(
            spacing: Space.s,
            mainAxisSize: MainAxisSize.min,
            children: [
              if (icon is String && icon.isNotEmpty)
                AppViewIcon(name: icon, size: 24),
              Text(
                string(c['title'], scope),
                textAlign: TextAlign.center,
                style: type.emphasis.copyWith(color: palette.text),
              ),
              if (body.isNotEmpty)
                Text(
                  body,
                  textAlign: TextAlign.center,
                  style: type.body.copyWith(color: palette.textMuted),
                ),
            ],
          ),
        );
    }
    return const SizedBox.shrink();
  }

  Widget _child(BuildContext context, Object? id, String scope, int depth) =>
      id is String
      ? node(context, id, scope, depth + 1)
      : const SizedBox.shrink();

  /// A ChildList: ids, or a template drawn once for each item of a list
  /// (or each entry of an object) in the data model, in that item's scope.
  List<Widget> _children(
    BuildContext context,
    Object? list,
    String scope,
    int depth,
  ) => [for (final (child, _) in _weighted(context, list, scope, depth)) child];

  /// Whether Button [id] shows words: a Text, or one in its List.
  bool _hasWords(String id) {
    final child = surface.components[surface.components[id]?['child']];
    return switch (child?['component']) {
      'Text' => true,
      'List' => switch (child!['children']) {
        final List ids => ids.any(
          (id) => surface.components[id]?['component'] == 'Text',
        ),
        _ => false,
      },
      _ => false,
    };
  }

  /// A bar button's words and whether they're a heading: its Text's, or
  /// its List's first Text's; null for one of icons alone.
  (String, bool)? _barLabel(String id, String scope) {
    final child = surface.components[surface.components[id]?['child']];
    final text = switch (child?['component']) {
      'Text' => child,
      'List' => switch (child!['children']) {
        final List ids =>
          ids
              .map((id) => surface.components[id])
              .where((c) => c?['component'] == 'Text')
              .firstOrNull,
        _ => null,
      },
      _ => null,
    };
    if (text == null) return null;
    return (string(text['text'], scope), text['variant'] == 'heading');
  }

  /// A Stat's or Map's thresholds, when they're a list of steps.
  static List<StatStep>? _thresholds(Object? value) => switch (value) {
    final List steps when steps.isNotEmpty && steps.length <= 10 => [
      for (final (i, step) in steps.indexed)
        if (step is Map && step['color'] is String)
          (
            value: i == 0
                ? null
                : switch (step['value']) {
                    final num n when n.isFinite => n.toDouble(),
                    _ => null,
                  },
            color: step['color']! as String,
          ),
    ],
    _ => null,
  };

  /// Component [id]'s weight as a flex, when it has one.
  int? _weightOf(String id) => switch (surface.components[id]?['weight']) {
    final num weight when weight > 0 => math.max(1, (weight * 100).round()),
    _ => null,
  };

  /// [_children], each with its weight (a flex) when it has one.
  List<(Widget, int?)> _weighted(
    BuildContext context,
    Object? list,
    String scope,
    int depth,
  ) {
    (Widget, int?) one(String id, String scope) =>
        (node(context, id, scope, depth + 1), _weightOf(id));

    if (list is List) {
      return [
        for (final id in list)
          if (id is String) one(id, scope),
      ];
    }
    if (list is Map<String, Object?> &&
        list['componentId'] is String &&
        list['path'] is String) {
      final id = list['componentId']! as String;
      final at = scopedPointer(list['path']! as String, scope);
      final items = surface.read(at);
      final keys = items is List
          ? [for (var i = 0; i < items.length; i++) '$i']
          : items is Map<String, Object?>
          ? [for (final key in items.keys) pointerStep(key)]
          : const <String>[];
      return [
        for (final key in keys.take(_maxTemplateItems)) one(id, '$at/$key'),
        if (keys.length > _maxTemplateItems)
          (_More(count: keys.length - _maxTemplateItems), null),
      ];
    }
    return const [];
  }

  Widget _button(
    BuildContext context,
    String id,
    Map<String, Object?> c,
    String scope,
    int depth, {
    bool bar = false,
  }) {
    final action = c['action'];
    final event = action is Map<String, Object?> ? action['event'] : null;
    final act = onAction;
    final onPressed = act != null && event is Map<String, Object?>
        ? () => act(id, event, scope)
        : null;
    // With a menu: it opens the menu over the page rather than sending
    // its own action, which is for apps that can't open one.
    final menu = _menu(c['menu'], scope);
    if (menu != null && act != null) {
      final opener = AppMenuOpener();
      return AppViewMenu(
        opener: opener,
        title: menu.title,
        options: menu.options,
        onChoose: (event) => act(id, event, scope),
        child: _buttonFace(context, id, c, scope, depth, opener.open, bar: bar),
      );
    }
    return _buttonFace(context, id, c, scope, depth, onPressed, bar: bar);
  }

  /// A Button's menu: its title and options, or null when it has none
  /// (or none of its options has an event).
  ({String title, List<AppMenuOption> options})? _menu(
    Object? menu,
    String scope,
  ) {
    if (menu is! Map<String, Object?>) return null;
    final options = <AppMenuOption>[
      if (menu['options'] case final List options)
        for (final option in options.take(500))
          if (option case {
            'action': {'event': final Map<String, Object?> event},
          })
            (
              label: string(option['label'], scope),
              selected: boolean(option['selected'], scope),
              event: event,
              icon: string(option['icon'], scope),
              section: string(option['section'], scope),
            ),
    ];
    if (options.isEmpty) return null;
    return (title: string(menu['title'], scope), options: options);
  }

  Widget _buttonFace(
    BuildContext context,
    String id,
    Map<String, Object?> c,
    String scope,
    int depth,
    VoidCallback? onPressed, {
    bool bar = false,
  }) {
    final childId = c['child'];
    final child = childId is String ? surface.components[childId] : null;
    // A Button's words are its Text child's, drawn as the app's buttons.
    if (child != null && child['component'] == 'Text') {
      final label = string(child['text'], scope);
      if (c['variant'] == 'borderless') {
        return Align(
          alignment: Alignment.centerLeft,
          widthFactor: _inLine || bar ? 1 : null,
          child: _QuietButton(label: label, onPressed: onPressed, oneLine: bar),
        );
      }
      return Align(
        alignment: Alignment.centerLeft,
        widthFactor: _inLine || bar ? 1 : null,
        child: Button(
          label: label,
          primary: c['variant'] == 'primary',
          onPressed: onPressed,
        ),
      );
    }
    // A row of a menu: a List holding one Text. Full width, its words
    // wrapping; the chosen one ("primary") filled, with an accent bar on
    // its left, as Grafana's pickers mark theirs.
    if (child?['children'] case [final String only]
        when !bar &&
            child!['component'] == 'List' &&
            surface.components[only]?['component'] == 'Text') {
      return _MenuRow(
        chosen: c['variant'] == 'primary',
        onPressed: onPressed,
        child: _child(context, only, scope, depth),
      );
    }
    final borderless = c['variant'] == 'borderless';
    return Semantics(
      button: true,
      enabled: onPressed != null,
      child: Pressable(
        onTap: onPressed,
        builder: (context, hovered) {
          final tint = BoxDecoration(
            color: hovered && onPressed != null
                ? Palette.of(context).hover
                : null,
            borderRadius: BorderRadius.circular(Radii.control),
          );
          final outer = _oneLine;
          _oneLine = bar;
          final inside = _child(context, childId, scope, depth);
          _oneLine = outer;
          if (!borderless) {
            return Container(
              padding: const EdgeInsets.all(Space.xs),
              decoration: tint,
              child: inside,
            );
          }
          // A borderless one (a menu's button in a top bar): its words in
          // line with the page's, the tint reaching a little past them,
          // and at least a target tall.
          return ConstrainedBox(
            constraints: BoxConstraints(minHeight: Metrics.of(context).target),
            child: Stack(
              clipBehavior: Clip.none,
              alignment: AlignmentDirectional.centerStart,
              children: [
                PositionedDirectional(
                  start: -Space.xs,
                  end: -Space.xs,
                  top: 0,
                  bottom: 0,
                  child: DecoratedBox(decoration: tint),
                ),
                Padding(
                  padding: const EdgeInsets.symmetric(vertical: Space.xs),
                  child: inside,
                ),
              ],
            ),
          );
        },
      ),
    );
  }

  Widget _choicePicker(
    BuildContext context,
    Map<String, Object?> c,
    String scope,
  ) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final label = string(c['label'], scope);
    final value = c['value'];
    final pointer = _pointer(value, scope);
    final chosen = stringList(value, scope);
    final single = c['variant'] != 'multipleSelection';
    final options = [
      if (c['options'] case final List options)
        for (final option in options.take(200))
          if (option is Map<String, Object?> && option['value'] is String)
            (string(option['label'], scope), option['value']! as String),
    ];
    final choose = onAction == null || pointer == null
        ? null
        : (String option) {
            final next = single
                ? [option]
                : chosen.contains(option)
                ? [
                    for (final one in chosen)
                      if (one != option) one,
                  ]
                : [...chosen, option];
            surface.write(pointer, next);
          };
    final chips = c['displayStyle'] == 'chips';
    final items = [
      for (final (text, option) in options)
        _Choice(
          label: text,
          chosen: chosen.contains(option),
          single: single,
          chip: chips,
          onTap: choose == null ? null : () => choose(option),
        ),
    ];
    return Column(
      spacing: Space.xs,
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (label.isNotEmpty)
          Text(label, style: type.caption.copyWith(color: palette.textMuted)),
        if (chips)
          Wrap(spacing: Space.xs, runSpacing: Space.xs, children: items)
        else
          ...items,
      ],
    );
  }

  /// `@accessibility`: a label and description for assistive technology,
  /// hidden from it, or read as it changes.
  Widget _accessible(Map<String, Object?> c, String scope, Widget built) {
    final a11y = c['@accessibility'];
    if (a11y is! Map<String, Object?>) return built;
    if (boolean(a11y['hidden'], scope)) return ExcludeSemantics(child: built);
    final label = string(a11y['label'], scope);
    final description = string(a11y['description'], scope);
    final live = a11y['live'] == 'polite' || a11y['live'] == 'assertive';
    if (label.isEmpty && description.isEmpty && !live) return built;
    return Semantics(
      label: label.isEmpty ? null : label,
      hint: description.isEmpty ? null : description,
      liveRegion: live,
      child: built,
    );
  }

  String? _pointer(Object? value, String scope) =>
      value is Map<String, Object?> && value['@path'] is String
      ? scopedPointer(value['@path']! as String, scope)
      : null;

  /// A DynString's text; other JSON as JSON, nothing as empty.
  String string(Object? value, String scope) =>
      appViewString(resolveValue(surface, value, scope));

  double? number(Object? value, String scope) =>
      switch (resolveValue(surface, value, scope)) {
        final num n => n.toDouble(),
        final String s => double.tryParse(s),
        _ => null,
      };

  bool boolean(Object? value, String scope) =>
      switch (resolveValue(surface, value, scope)) {
        final bool b => b,
        final Map<String, Object?> result => result['valid'] == true,
        final String s => s.isNotEmpty && s != 'false',
        final num n => n != 0,
        _ => false,
      };

  List<String> stringList(Object? value, String scope) =>
      switch (resolveValue(surface, value, scope)) {
        final List list => [for (final item in list) appViewString(item)],
        final String s when s.isNotEmpty => [s],
        _ => const [],
      };
}

/// A value as text: strings as they are, numbers and booleans written
/// out, objects and lists as JSON, nothing as empty.
String appViewString(Object? value) => switch (value) {
  null => '',
  final String s => s,
  final num n when n == n.roundToDouble() && n.abs() < 1e15 => '${n.toInt()}',
  final num n => '$n',
  final bool b => '$b',
  _ => jsonEncode(value),
};

MainAxisAlignment _justify(Object? justify) => switch (justify) {
  'center' => MainAxisAlignment.center,
  'end' => MainAxisAlignment.end,
  'spaceBetween' => MainAxisAlignment.spaceBetween,
  'spaceAround' => MainAxisAlignment.spaceAround,
  'spaceEvenly' => MainAxisAlignment.spaceEvenly,
  _ => MainAxisAlignment.start,
};

CrossAxisAlignment _align(Object? align, CrossAxisAlignment otherwise) =>
    switch (align) {
      'start' => CrossAxisAlignment.start,
      'center' => CrossAxisAlignment.center,
      'end' => CrossAxisAlignment.end,
      'stretch' => CrossAxisAlignment.stretch,
      _ => otherwise,
    };

/// An Icon component: the Lucide icon it names, from the app's own set, at
/// 16px in `textMuted`; a name this app can't draw shows a generic dot.
/// Nothing is ever loaded by name or address.
class AppViewIcon extends StatelessWidget {
  const AppViewIcon({super.key, required this.name, this.size = 16});

  final String name;
  final double size;

  @override
  Widget build(BuildContext context) => ExcludeSemantics(
    // One it can't draw is a generic dot: never loaded from anywhere.
    child: ExtensionGlyph(
      icon: name,
      name: '•',
      color: Palette.of(context).textMuted,
      size: size,
    ),
  );
}

/// A borderless Button: its words in label weight, `textMuted`, `text` on
/// hover.
class _QuietButton extends StatelessWidget {
  const _QuietButton({
    required this.label,
    required this.onPressed,
    this.oneLine = false,
  });

  final String label;
  final VoidCallback? onPressed;

  /// On a bar: one line, ending in "…" when there isn't room.
  final bool oneLine;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      enabled: onPressed != null,
      label: label,
      onTap: onPressed,
      excludeSemantics: true,
      child: Pressable(
        onTap: onPressed,
        builder: (context, hovered) => Padding(
          padding: const EdgeInsets.symmetric(vertical: Space.xs),
          child: Text(
            label,
            maxLines: oneLine ? 1 : null,
            softWrap: !oneLine,
            overflow: oneLine ? TextOverflow.ellipsis : null,
            style: TypeScale.of(context).label.copyWith(
              color: onPressed == null
                  ? palette.textFaint
                  : hovered
                  ? palette.text
                  : palette.textMuted,
            ),
          ),
        ),
      ),
    );
  }
}

/// Whether [component] fills a grid's gap: a weighted Column with no
/// children (not one bound to data).
bool _isGap(Map<String, Object?>? component) =>
    component != null &&
    component['component'] == 'Column' &&
    component['weight'] is num &&
    switch (component['children']) {
      final List children => children.isEmpty,
      _ => false,
    };

/// A row of a menu (a Button over a List of one Text): full width, a
/// finger's height on a phone, its words wrapping; the [chosen] one
/// filled, with an accent bar on its left, as Grafana's pickers mark
/// theirs. Focusable, Return or Space presses it.
class _MenuRow extends StatefulWidget {
  const _MenuRow({
    required this.chosen,
    required this.onPressed,
    required this.child,
  });

  final bool chosen;
  final VoidCallback? onPressed;
  final Widget child;

  @override
  State<_MenuRow> createState() => _MenuRowState();
}

class _MenuRowState extends State<_MenuRow> {
  var _hovered = false;
  var _focused = false;

  static const _bar = 3.0;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final enabled = widget.onPressed != null;
    final chosen = widget.chosen;
    return Semantics(
      button: true,
      selected: chosen,
      enabled: enabled,
      child: FocusableActionDetector(
        enabled: enabled,
        mouseCursor: enabled
            ? SystemMouseCursors.click
            : SystemMouseCursors.basic,
        onShowHoverHighlight: (value) => setState(() => _hovered = value),
        onShowFocusHighlight: (value) => setState(() => _focused = value),
        actions: {
          ActivateIntent: CallbackAction<ActivateIntent>(
            onInvoke: (_) {
              widget.onPressed?.call();
              return null;
            },
          ),
        },
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: widget.onPressed,
          child: LayoutBuilder(
            builder: (context, constraints) => Container(
              width: constraints.hasBoundedWidth ? double.infinity : null,
              constraints: BoxConstraints(
                minHeight: Metrics.of(context).target,
              ),
              alignment: Alignment.centerLeft,
              // The bar takes its width from the left inset, so the
              // chosen row's words line up with the others'.
              padding: EdgeInsets.fromLTRB(
                Space.m - (chosen ? _bar : 0),
                Space.s - 2,
                Space.m,
                Space.s - 2,
              ),
              decoration: BoxDecoration(
                color: chosen
                    ? palette.selected
                    : (_hovered || _focused) && enabled
                    ? palette.hover
                    : null,
                border: chosen
                    ? Border(
                        left: BorderSide(color: palette.signal, width: _bar),
                      )
                    : null,
              ),
              child: widget.child,
            ),
          ),
        ),
      ),
    );
  }
}

/// The least each of a weighted Row's children gets side by side, at the
/// standard text size (more as the text grows); narrower, they go one
/// under another.
const minRowChildWidth = 120.0;

/// One of a ChoicePicker's options: a row with a round (one of them) or
/// square (any of them) mark, or a pill when drawn as chips; chosen, the
/// mark fills in `signal` and a chip in `selected`.
class _Choice extends StatelessWidget {
  const _Choice({
    required this.label,
    required this.chosen,
    required this.single,
    required this.chip,
    required this.onTap,
  });

  final String label;
  final bool chosen;
  final bool single;
  final bool chip;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final phone = touchFirst(context);
    return Semantics(
      button: true,
      selected: chosen,
      inMutuallyExclusiveGroup: single,
      enabled: onTap != null,
      label: label,
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) {
          if (chip) {
            return AnimatedContainer(
              duration: Motion.fast,
              constraints: BoxConstraints(minHeight: phone ? 32 : 24),
              padding: const EdgeInsets.symmetric(horizontal: Space.s + 2),
              decoration: BoxDecoration(
                color: chosen
                    ? palette.selected
                    : hovered
                    ? palette.hover
                    : null,
                border: Border.all(
                  color: chosen ? const Color(0x00000000) : palette.hairline,
                ),
                borderRadius: BorderRadius.circular(999),
              ),
              // As wide as its words, in a Wrap with the others.
              child: Center(
                widthFactor: 1,
                child: Text(
                  label,
                  style: type.caption.copyWith(
                    color: chosen ? palette.text : palette.textMuted,
                    fontWeight: chosen ? FontWeight.w600 : null,
                  ),
                ),
              ),
            );
          }
          return Container(
            constraints: BoxConstraints(minHeight: phone ? Touch.target : 28),
            padding: const EdgeInsets.symmetric(horizontal: Space.xs),
            decoration: BoxDecoration(
              color: hovered && onTap != null ? palette.hover : null,
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            child: Row(
              spacing: Space.s,
              children: [
                AnimatedContainer(
                  duration: Motion.fast,
                  width: 14,
                  height: 14,
                  decoration: BoxDecoration(
                    color: chosen ? palette.signal : null,
                    border: Border.all(
                      color: chosen ? palette.signal : palette.textFaint,
                    ),
                    borderRadius: BorderRadius.circular(single ? 7 : 3),
                  ),
                ),
                Expanded(
                  child: Text(
                    label,
                    style: type.body.copyWith(color: palette.text),
                  ),
                ),
              ],
            ),
          );
        },
      ),
    );
  }
}

/// A TextField bound to the data model: what's typed is written there, and
/// what the app puts there shows, unless it's being typed in. Its note
/// names the app it goes to, and nothing typed is ever hidden as a
/// password: catalog v1 has no such field.
class _BoundTextField extends StatefulWidget {
  const _BoundTextField({
    super.key,
    required this.surface,
    required this.value,
    required this.scope,
    required this.label,
    required this.placeholder,
    required this.variant,
    required this.enabled,
    required this.appName,
  });

  final AppSurfaceModel surface;
  final Object? value;
  final String scope;
  final String label;
  final String placeholder;
  final String variant;
  final bool enabled;

  /// Whose field it is: what's typed goes to that app.
  final String appName;

  @override
  State<_BoundTextField> createState() => _BoundTextFieldState();
}

class _BoundTextFieldState extends State<_BoundTextField> {
  late final _text = TextEditingController(text: _current);
  final _focus = FocusNode();

  String? get _pointer =>
      widget.value is Map<String, Object?> &&
          (widget.value! as Map<String, Object?>)['@path'] is String
      ? scopedPointer(
          (widget.value! as Map<String, Object?>)['@path']! as String,
          widget.scope,
        )
      : null;

  String get _current =>
      appViewString(resolveValue(widget.surface, widget.value, widget.scope));

  @override
  void initState() {
    super.initState();
    _text.addListener(_typed);
  }

  void _typed() {
    final pointer = _pointer;
    if (pointer == null || _text.text == _current) return;
    final number = widget.variant == 'number';
    widget.surface.write(
      pointer,
      number ? (num.tryParse(_text.text) ?? _text.text) : _text.text,
    );
  }

  @override
  void didUpdateWidget(_BoundTextField old) {
    super.didUpdateWidget(old);
    final current = _current;
    if (!_focus.hasFocus && _text.text != current) _text.text = current;
  }

  @override
  void dispose() {
    _text.dispose();
    _focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final long = widget.variant == 'longText';
    final field = TextField(
      controller: _text,
      focusNode: _focus,
      placeholder: widget.placeholder,
      minLines: long ? 3 : null,
      maxLines: long ? 8 : 1,
      keyboardType: widget.variant == 'number'
          ? const TextInputType.numberWithOptions(decimal: true, signed: true)
          : null,
      inputFormatters: widget.variant == 'number'
          ? [FilteringTextInputFormatter.allow(RegExp(r'[0-9.,eE+-]'))]
          : null,
      semanticsLabel: widget.label,
    );
    // Always said whose it is: an app's field never passes for
    // Claktory's own.
    return IgnorePointer(
      ignoring: !widget.enabled,
      child: Field(
        label: widget.label.isEmpty ? widget.appName : widget.label,
        // Said once above a form's fields instead (_FieldsNoted).
        note: _FieldsNoted.on(context) ? null : fieldNote(widget.appName),
        child: field,
      ),
    );
  }
}

/// What an app's field says, under it, about what's typed in it.
String fieldNote(String appName) => '$appName gets what you type here.';

/// The same, said once above a form's fields.
String fieldsNote(String appName) => '$appName gets what you type below.';

/// Over a form whose fields' note is said once, above them.
class _FieldsNoted extends InheritedWidget {
  const _FieldsNoted({required super.child});

  static bool on(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<_FieldsNoted>() != null;

  @override
  bool updateShouldNotify(_FieldsNoted old) => false;
}

/// A Slider bound to the data model: a 4px track in `text` at 14%, filled
/// in `signal` to a 16px thumb, with the value at its right; dragging or
/// clicking sets it, and assistive technology steps it by a tenth.
class _BoundSlider extends StatelessWidget {
  const _BoundSlider({
    required this.surface,
    required this.value,
    required this.scope,
    required this.label,
    required this.min,
    required this.max,
    required this.enabled,
  });

  final AppSurfaceModel surface;
  final Object? value;
  final String scope;
  final String label;
  final double min;
  final double max;
  final bool enabled;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final range = max > min ? max - min : 1.0;
    final raw = resolveValue(surface, value, scope);
    final now = (raw is num ? raw.toDouble() : min).clamp(min, max);
    final at = (now - min) / range;
    final pointer =
        value is Map<String, Object?> &&
            (value! as Map<String, Object?>)['@path'] is String
        ? scopedPointer(
            (value! as Map<String, Object?>)['@path']! as String,
            scope,
          )
        : null;
    final set = !enabled || pointer == null
        ? null
        : (double next) => surface.write(pointer, next.clamp(min, max));
    final step = range / 10;
    final shown = appViewString(
      double.parse(now.toStringAsFixed(range >= 10 ? 0 : 2)),
    );
    return Semantics(
      slider: true,
      label: label.isEmpty ? null : label,
      value: shown,
      increasedValue: appViewString(math.min(max, now + step)),
      decreasedValue: appViewString(math.max(min, now - step)),
      onIncrease: set == null ? null : () => set(now + step),
      onDecrease: set == null ? null : () => set(now - step),
      child: ExcludeSemantics(
        child: Column(
          spacing: Space.xs,
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (label.isNotEmpty)
              Text(
                label,
                style: type.caption.copyWith(color: palette.textMuted),
              ),
            Row(
              spacing: Space.m,
              children: [
                Expanded(
                  child: LayoutBuilder(
                    builder: (context, constraints) {
                      final width = math.max(1.0, constraints.maxWidth - 16);
                      void to(double x) => set?.call(
                        min + ((x - 8) / width).clamp(0, 1) * range,
                      );
                      return GestureDetector(
                        behavior: HitTestBehavior.opaque,
                        onTapDown: (details) => to(details.localPosition.dx),
                        onHorizontalDragUpdate: (details) =>
                            to(details.localPosition.dx),
                        child: SizedBox(
                          height: touchFirst(context) ? Touch.target : 24,
                          child: Stack(
                            alignment: Alignment.centerLeft,
                            children: [
                              Padding(
                                padding: const EdgeInsets.symmetric(
                                  horizontal: 8,
                                ),
                                child: MoveMeter(value: at.toDouble()),
                              ),
                              Positioned(
                                left: at * width,
                                child: Container(
                                  width: 16,
                                  height: 16,
                                  decoration: BoxDecoration(
                                    color: palette.canvas,
                                    shape: BoxShape.circle,
                                    border: Border.all(
                                      color: set == null
                                          ? palette.textFaint
                                          : palette.signal,
                                      width: 2,
                                    ),
                                  ),
                                ),
                              ),
                            ],
                          ),
                        ),
                      );
                    },
                  ),
                ),
                Text(
                  shown,
                  style: type.caption.copyWith(
                    color: palette.textMuted,
                    fontFeatures: const [FontFeature.tabularFigures()],
                  ),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

/// Tabs: their titles as pills, the chosen one in `selected` (which one is
/// chosen stays on this device), and its child under them.
class _Tabs extends StatefulWidget {
  const _Tabs({super.key, required this.titles, required this.child});

  final List<String> titles;
  final Widget Function(int index) child;

  @override
  State<_Tabs> createState() => _TabsState();
}

class _TabsState extends State<_Tabs> {
  var _chosen = 0;

  @override
  Widget build(BuildContext context) {
    final titles = widget.titles;
    if (titles.isEmpty) return const SizedBox.shrink();
    final chosen = _chosen.clamp(0, titles.length - 1);
    return Column(
      spacing: Space.m,
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Wrap(
          spacing: Space.xs,
          runSpacing: Space.xs,
          children: [
            for (var i = 0; i < titles.length; i++)
              _Choice(
                label: titles[i],
                chosen: i == chosen,
                single: true,
                chip: true,
                onTap: () => setState(() => _chosen = i),
              ),
          ],
        ),
        widget.child(chosen),
      ],
    );
  }
}

/// Under a template cut short: how many more items it has.
class _More extends StatelessWidget {
  const _More({required this.count});

  final int count;

  @override
  Widget build(BuildContext context) => Text(
    'and $count more',
    style: TypeScale.of(context).caption
        .copyWith(color: Palette.of(context).textFaint),
  );
}

/// A bar of buttons on one line ([_Renderer]'s Row of Buttons with a
/// weighted one: a dashboard's name, range, filters and •••): the
/// unweighted ones at their own width, the weighted ones sharing the rest
/// by weight, so the last stays at the line's end. The weighted ones give
/// way first, down to [leastButton] (their first letters and "…": the
/// dashboard's name says where you are), or [leastWeighted] for one
/// without words; only where the others don't fit even then (large text
/// on a small phone) do they shrink, in proportion to what they have
/// above it, and where even every button's least doesn't fit, all give
/// way evenly. Each button's words end in "…" where its room ends. Where
/// a button would keep less than three letters and "…" (large text on a
/// phone), the weighted ones take a line of their own and the
/// others go under it, as Grafana's phone bar puts its title above its
/// controls.
class _BarLayout extends MultiChildRenderObjectWidget {
  const _BarLayout({
    required this.weights,
    required this.hasWords,
    required this.labels,
    required super.children,
  });

  /// Each child's words and whether they're a heading, if it has words.
  final List<(String, bool)?> labels;

  /// Each child's weight, null for one at its own width.
  final List<int?> weights;

  /// Whether each child has words, which can end in "…".
  final List<bool> hasWords;

  /// The least a button with words gives way to at the normal text size,
  /// grown with it: its first letters and "…".
  static const leastButton = 48.0;

  /// The least a weighted one without words gives way to: a chevron and
  /// its gap.
  static const leastWeighted = 24.0;

  /// [leastButton] at the text size: first letters at any size.
  static double _least(BuildContext context) =>
      MediaQuery.textScalerOf(context).scale(leastButton);

  /// What a button's words and chevron take besides its words: its
  /// padding, the chevron and the gap before it.
  static const buttonChrome = 24.0;

  /// How many letters a button with words keeps before its "…" on one
  /// line: one letter doesn't say where you are.
  static const readableLetters = 3;

  /// The least each button with words keeps its first
  /// [readableLetters] letters (spaces don't count) and "…" in at the
  /// text size ("Cla…", "30 d…"): under it the bar takes more lines (see
  /// [_RenderBar._lines]). Infinite for one without words.
  List<double> _readable(BuildContext context) {
    final type = TypeScale.of(context);
    return [
      for (final label in labels)
        if (label case (final text, final heading)?)
          _width(
            context,
            _readablePart(text),
            heading
                ? type.emphasis.copyWith(fontWeight: FontWeight.w600)
                : type.body,
          )
        else
          double.infinity,
    ];
  }

  /// [text] up to its [readableLetters]th letter, and "…" if more
  /// follows.
  static String _readablePart(String text) {
    var letters = 0;
    final runes = text.runes.toList();
    for (var k = 0; k < runes.length; k++) {
      if (String.fromCharCode(runes[k]).trim().isEmpty) continue;
      letters++;
      if (letters == readableLetters) {
        return k == runes.length - 1
            ? text
            : '${String.fromCharCodes(runes.take(k + 1))}…';
      }
    }
    return text;
  }

  static double _width(BuildContext context, String text, TextStyle style) {
    final painter = TextPainter(
      text: TextSpan(text: text, style: style),
      textScaler: MediaQuery.textScalerOf(context),
      textDirection: Directionality.of(context),
      maxLines: 1,
    )..layout();
    final width = painter.width;
    painter.dispose();
    // A little slack: the button's own chrome is a hair over its round
    // number, and "…" fits only on room it wholly has.
    return width.ceilToDouble() + buttonChrome + 2;
  }

  @override
  RenderObject createRenderObject(BuildContext context) => _RenderBar(
    weights,
    hasWords,
    Directionality.of(context),
    _least(context),
    _readable(context),
    Metrics.of(context).target,
  );

  @override
  void updateRenderObject(BuildContext context, _RenderBar renderObject) {
    renderObject
      ..weights = weights
      ..hasWords = hasWords
      ..direction = Directionality.of(context)
      ..least = _least(context)
      ..readable = _readable(context)
      ..target = Metrics.of(context).target;
  }
}

class _BarParentData extends ContainerBoxParentData<RenderBox> {}

/// A dashboard's row of panels: each its weight's share of the width, less
/// [spacing] between, and every one as tall as the tallest.
class _EvenRow extends MultiChildRenderObjectWidget {
  const _EvenRow({
    required this.weights,
    required this.spacing,
    required super.children,
  });

  final List<int> weights;
  final double spacing;

  @override
  _RenderEvenRow createRenderObject(BuildContext context) =>
      _RenderEvenRow(weights, spacing, Directionality.of(context));

  @override
  void updateRenderObject(BuildContext context, _RenderEvenRow row) => row
    ..weights = weights
    ..spacing = spacing
    ..direction = Directionality.of(context);
}

class _RenderEvenRow extends RenderBox
    with
        ContainerRenderObjectMixin<RenderBox, _BarParentData>,
        RenderBoxContainerDefaultsMixin<RenderBox, _BarParentData> {
  _RenderEvenRow(this._weights, this._spacing, this._direction);

  List<int> _weights;
  set weights(List<int> value) {
    if (listEquals(value, _weights)) return;
    _weights = value;
    markNeedsLayout();
  }

  double _spacing;
  set spacing(double value) {
    if (value == _spacing) return;
    _spacing = value;
    markNeedsLayout();
  }

  TextDirection _direction;
  set direction(TextDirection value) {
    if (value == _direction) return;
    _direction = value;
    markNeedsLayout();
  }

  @override
  void setupParentData(RenderBox child) {
    if (child.parentData is! _BarParentData) {
      child.parentData = _BarParentData();
    }
  }

  @override
  void performLayout() {
    final kids = getChildrenAsList();
    final width = constraints.maxWidth;
    final total = _weights.take(kids.length).fold(0, (sum, w) => sum + w);
    final room = math.max(0.0, width - _spacing * (kids.length - 1));
    final widths = [
      for (var i = 0; i < kids.length; i++)
        total == 0 || i >= _weights.length ? 0.0 : room * _weights[i] / total,
    ];
    // Each at its own height, then the shorter ones again at the tallest's.
    var tallest = 0.0;
    for (final (i, kid) in kids.indexed) {
      kid.layout(
        BoxConstraints.tightFor(width: widths[i]),
        parentUsesSize: true,
      );
      tallest = math.max(tallest, kid.size.height);
    }
    tallest = constraints.constrainHeight(tallest);
    var x = 0.0;
    for (final (i, kid) in kids.indexed) {
      if (kid.size.height != tallest) {
        kid.layout(
          BoxConstraints.tightFor(width: widths[i], height: tallest),
          parentUsesSize: true,
        );
      }
      (kid.parentData! as _BarParentData).offset = Offset(
        _direction == TextDirection.rtl ? width - x - widths[i] : x,
        0,
      );
      x += widths[i] + _spacing;
    }
    size = constraints.constrain(Size(width, tallest));
  }

  @override
  bool hitTestChildren(BoxHitTestResult result, {required Offset position}) =>
      defaultHitTestChildren(result, position: position);

  @override
  void paint(PaintingContext context, Offset offset) =>
      defaultPaint(context, offset);
}

/// A render object that takes taps in the page's margin beside it: a
/// view's page retries a tap there that hit nothing on the nearest point
/// of its content, and keeps it only where one of these takes it (a bar's
/// •••, a finger's target wide though drawn narrower).
abstract interface class EdgeTapTarget {}

class _RenderBar extends RenderBox
    with
        ContainerRenderObjectMixin<RenderBox, _BarParentData>,
        RenderBoxContainerDefaultsMixin<RenderBox, _BarParentData>
    implements EdgeTapTarget {
  _RenderBar(
    this._weights,
    this._hasWords,
    this._direction,
    this._least,
    this._readable,
    this._target,
  );

  /// A finger's target: the least a button of icons alone answers taps
  /// across, centred on it, whatever it's drawn at.
  double _target;
  set target(double value) {
    if (value == _target) return;
    _target = value;
  }

  double _least;
  set least(double value) {
    if (value == _least) return;
    _least = value;
    markNeedsLayout();
  }

  List<double> _readable;
  set readable(List<double> value) {
    if (listEquals(value, _readable)) return;
    _readable = value;
    markNeedsLayout();
  }

  List<bool> _hasWords;
  set hasWords(List<bool> value) {
    if (listEquals(value, _hasWords)) return;
    _hasWords = value;
    markNeedsLayout();
  }

  List<int?> _weights;
  set weights(List<int?> value) {
    if (listEquals(value, _weights)) return;
    _weights = value;
    markNeedsLayout();
  }

  TextDirection _direction;
  set direction(TextDirection value) {
    if (value == _direction) return;
    _direction = value;
    markNeedsLayout();
  }

  static const _gap = Space.s;

  @override
  void setupParentData(RenderBox child) {
    if (child.parentData is! _BarParentData) {
      child.parentData = _BarParentData();
    }
  }

  List<RenderBox> get _children {
    final out = <RenderBox>[];
    var child = firstChild;
    while (child != null) {
      out.add(child);
      child = childAfter(child);
    }
    return out;
  }

  int? _weight(int index) => index < _weights.length ? _weights[index] : null;

  bool _words(int index) => index < _hasWords.length && _hasWords[index];

  @override
  double computeMinIntrinsicWidth(double height) =>
      computeMaxIntrinsicWidth(height);

  @override
  double computeMaxIntrinsicWidth(double height) {
    final children = _children;
    return children.fold(
          0.0,
          (sum, child) => sum + child.getMaxIntrinsicWidth(double.infinity),
        ) +
        _gap * math.max(0, children.length - 1);
  }

  @override
  double computeMinIntrinsicHeight(double width) => _height(
    width,
    (child, _) => child.getMinIntrinsicHeight(double.infinity),
  );

  @override
  double computeMaxIntrinsicHeight(double width) => _height(
    width,
    (child, _) => child.getMaxIntrinsicHeight(double.infinity),
  );

  /// The bar's height on a line [width] wide: its lines', each as tall as
  /// its tallest child ([tall] of a child at its width), [_gap] apart.
  double _height(double width, double Function(RenderBox, double) tall) {
    final children = _children;
    final lines = _lines(width);
    var height = 0.0;
    for (final line in lines) {
      height += line.fold(
        0.0,
        (most, slot) => math.max(most, tall(children[slot.$1], slot.$2)),
      );
    }
    return height + _gap * math.max(0, lines.length - 1);
  }

  List<double> get _natural => [
    for (final child in _children) child.getMaxIntrinsicWidth(double.infinity),
  ];

  /// The bar's lines on [width]: each child's index and width. One line
  /// while each button with words keeps a few letters and "…" on it;
  /// where it wouldn't (the largest text on a small phone), the weighted
  /// ones (the dashboard's name) on a line of their own, as Grafana's
  /// title is, and the others under it, on as many lines as they need to
  /// keep theirs.
  List<List<(int, double)>> _lines(double width) {
    final natural = _natural;
    final all = [for (var i = 0; i < natural.length; i++) i];
    List<(int, double)> fit(List<int> line, {bool wrapped = false}) => [
      for (final (k, w) in _fit(line, natural, width, wrapped: wrapped).indexed)
        (line[k], w),
    ];
    final one = fit(all);
    if (!width.isFinite) return [one];
    double readable(int i) => _words(i) && i < _readable.length
        ? math.min(natural[i], _readable[i])
        : natural[i];
    final squeezed = one.any(
      (slot) => _words(slot.$1) && slot.$2 < readable(slot.$1) - 0.5,
    );
    final first = [
      for (final i in all)
        if (_weight(i) != null) i,
    ];
    if (!squeezed || first.isEmpty || first.length == all.length) return [one];
    final lines = [fit(first)];
    var line = <int>[];
    var used = 0.0;
    for (final i in all) {
      if (_weight(i) != null) continue;
      final need = readable(i);
      if (line.isNotEmpty && used + _gap + need > width) {
        lines.add(fit(line, wrapped: true));
        line = [];
        used = 0;
      }
      used += (line.isEmpty ? 0 : _gap) + need;
      line.add(i);
    }
    if (line.isNotEmpty) lines.add(fit(line, wrapped: true));
    return lines;
  }

  /// The widths of [line]'s children on a line [width] wide: the
  /// unweighted ones at their own width, the weighted ones sharing the
  /// rest, giving way as [_BarLayout] says.
  List<double> _fit(
    List<int> line,
    List<double> natural,
    double width, {
    bool wrapped = false,
  }) {
    if (!width.isFinite) return [for (final i in line) natural[i]];
    final room = math.max(0.0, width - _gap * math.max(0, line.length - 1));
    var fixed = 0.0;
    var total = 0;
    for (final i in line) {
      if (_weight(i) case final weight?) {
        total += weight;
      } else {
        fixed += natural[i];
      }
    }
    // The least each gives way to: words keep an icon or their first
    // letters and "…", a weighted one of icons alone its chevron, one of
    // icons alone its width.
    // On a line of its own, packed so each keeps its first letters, none
    // gives way past them.
    double floor(int i) => _words(i)
        ? math.min(
            natural[i],
            wrapped && i < _readable.length ? _readable[i] : _least,
          )
        : _weight(i) != null
        ? math.min(natural[i], _BarLayout.leastWeighted)
        : natural[i];
    // The weighted ones' share: the rest of the line, or on a short line
    // their least.
    var keep = 0.0;
    for (final i in line) {
      if (_weight(i) != null) keep += floor(i);
    }
    final shared = total == 0 ? 0.0 : math.max(room - fixed, keep);
    final left = room - shared;
    final widths = {for (final i in line) i: natural[i]};
    if (fixed > left) {
      // The others with words give way, each by what it has above its
      // least.
      final others = [
        for (final i in line)
          if (_weight(i) == null) i,
      ];
      final slack = others.fold(0.0, (sum, i) => sum + natural[i] - floor(i));
      final over = fixed - left;
      if (slack > 0) {
        final part = math.min(1.0, over / slack);
        for (final i in others) {
          widths[i] = natural[i] - (natural[i] - floor(i)) * part;
        }
      }
    }
    for (final i in line) {
      if (_weight(i) case final weight?) {
        widths[i] = shared * weight / total;
      }
    }
    // Where even their least doesn't fit, all that can give way do so
    // evenly; icons alone keep their width.
    final used = widths.values.fold(0.0, (sum, w) => sum + w);
    if (used > room) {
      bool gives(int i) => _words(i) || _weight(i) != null;
      var stays = 0.0;
      var giving = 0.0;
      for (final i in line) {
        if (gives(i)) {
          giving += widths[i]!;
        } else {
          stays += widths[i]!;
        }
      }
      final scale = giving == 0
          ? 0.0
          : (math.max(0.0, room - stays) / giving).clamp(0.0, 1.0);
      for (final i in line) {
        if (gives(i)) widths[i] = widths[i]! * scale;
      }
    }
    return [for (final i in line) widths[i]!];
  }

  /// The width the bar takes: its line's, or all of [width] on several.
  double _width(List<List<(int, double)>> lines, double width) =>
      lines.length > 1
      ? width
      : lines.single.fold(0.0, (sum, slot) => sum + slot.$2) +
            _gap * math.max(0, lines.single.length - 1);

  @override
  Size computeDryLayout(BoxConstraints constraints) {
    final lines = _lines(constraints.maxWidth);
    final height = _height(
      constraints.maxWidth,
      (child, width) =>
          child.getDryLayout(BoxConstraints(maxWidth: width)).height,
    );
    return constraints.constrain(
      Size(_width(lines, constraints.maxWidth), height),
    );
  }

  @override
  void performLayout() {
    final children = _children;
    final lines = _lines(constraints.maxWidth);
    final heights = <double>[];
    for (final line in lines) {
      var height = 0.0;
      for (final (i, width) in line) {
        children[i].layout(
          BoxConstraints(maxWidth: width, maxHeight: constraints.maxHeight),
          parentUsesSize: true,
        );
        height = math.max(height, children[i].size.height);
      }
      heights.add(height);
    }
    size = constraints.constrain(
      Size(
        _width(lines, constraints.maxWidth),
        heights.fold(0.0, (sum, h) => sum + h) +
            _gap * math.max(0, lines.length - 1),
      ),
    );
    // Left to right (right to left in a right-to-left language), each in
    // its slot, centred on its line; the last of icons alone (•••) at the
    // end of its line.
    var y = 0.0;
    for (final (l, line) in lines.indexed) {
      var x = 0.0;
      for (final (k, (i, slot)) in line.indexed) {
        final child = children[i];
        final last =
            lines.length > 1 &&
            k == line.length - 1 &&
            i == children.length - 1 &&
            !_words(i) &&
            _weight(i) == null;
        if (last) x = math.max(x, size.width - slot);
        final left = _direction == TextDirection.ltr
            ? x
            : size.width - x - child.size.width;
        (child.parentData! as _BarParentData).offset = Offset(
          left,
          y + (heights[l] - child.size.height) / 2,
        );
        x += slot + _gap;
      }
      y += heights[l] + _gap;
    }
  }

  @override
  void paint(PaintingContext context, Offset offset) =>
      defaultPaint(context, offset);

  @override
  bool hitTestChildren(BoxHitTestResult result, {required Offset position}) {
    if (defaultHitTestChildren(result, position: position)) return true;
    // Missed: a button of icons alone (•••) narrower than a finger's
    // target takes a tap within a target around it, reaching into the
    // gaps and the page's edge without moving anything or taking another
    // button's taps (Zucchero row 444).
    for (final (i, child) in _children.indexed) {
      if (_words(i) || _weight(i) != null) continue;
      final offset = (child.parentData! as _BarParentData).offset;
      final rect = offset & child.size;
      final reach = Rect.fromCenter(
        center: rect.center,
        width: math.max(rect.width, _target),
        height: math.max(rect.height, _target),
      );
      if (!reach.contains(position)) continue;
      return result.addWithPaintOffset(
        offset: offset,
        position: position,
        hitTest: (result, _) =>
            child.hitTest(result, position: child.size.center(Offset.zero)),
      );
    }
    return false;
  }

  /// How far past its sides the bar takes taps: half a target, for a
  /// button of icons alone at its end.
  double get _reach => _target / 2;

  @override
  bool hitTest(BoxHitTestResult result, {required Offset position}) {
    final reach = Rect.fromLTRB(
      -_reach,
      -_reach,
      size.width + _reach,
      size.height + _reach,
    );
    if (!reach.contains(position)) return false;
    if (hitTestChildren(result, position: position) ||
        (size.contains(position) && hitTestSelf(position))) {
      result.add(BoxHitTestEntry(this, position));
      return true;
    }
    return false;
  }
}
