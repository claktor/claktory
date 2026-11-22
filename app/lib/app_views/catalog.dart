import 'dart:convert';
import 'dart:math' as math;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import '../mobile/metrics.dart';
import '../move.dart' show MoveMeter;
import 'charts.dart';
import 'controller.dart';
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
  const _Renderer({
    required this.surface,
    required this.onAction,
    required this.appName,
  });

  final AppSurfaceModel surface;
  final AppViewAct? onAction;
  final String appName;

  Widget node(BuildContext context, String id, String scope, int depth) {
    final component = surface.components[id];
    if (component == null || depth > _maxDepth) return const SizedBox.shrink();
    final built = _component(context, id, component, scope, depth);
    return _accessible(component, scope, built);
  }

  Widget _component(
    BuildContext context,
    String id,
    Map<String, Object?> c,
    String scope,
    int depth,
  ) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    List<Widget> children(Object? list) =>
        _children(context, list, scope, depth);
    switch (c['component']) {
      case 'Text':
        final variant = c['variant'];
        final text = Text(
          string(c['text'], scope),
          style: switch (variant) {
            'heading' => type.emphasis.copyWith(
              color: palette.text,
              fontWeight: FontWeight.w600,
            ),
            'caption' => type.caption.copyWith(color: palette.textMuted),
            _ => type.body.copyWith(color: palette.text),
          },
        );
        return variant == 'heading'
            ? Semantics(header: true, child: text)
            : text;
      case 'Row':
        return Row(
          spacing: Space.s,
          mainAxisAlignment: _justify(c['justify']),
          crossAxisAlignment: _align(c['align'], CrossAxisAlignment.center),
          children: [
            for (final (child, weight) in _weighted(
              context,
              c['children'],
              scope,
              depth,
            ))
              weight == null
                  ? Flexible(child: child)
                  : Expanded(flex: weight, child: child),
          ],
        );
      case 'Column':
        return Column(
          spacing: Space.s,
          mainAxisSize: MainAxisSize.min,
          mainAxisAlignment: _justify(c['justify']),
          crossAxisAlignment: _align(c['align'], CrossAxisAlignment.stretch),
          children: children(c['children']),
        );
      case 'List':
        if (c['direction'] == 'horizontal') {
          return SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: Row(
              spacing: Space.s,
              crossAxisAlignment: _align(c['align'], CrossAxisAlignment.start),
              children: children(c['children']),
            ),
          );
        }
        return Column(
          spacing: Space.xs,
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: _align(c['align'], CrossAxisAlignment.stretch),
          children: children(c['children']),
        );
      case 'Card':
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
        return _button(context, id, c, scope, depth);
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
          color: color is String ? roleColor(context, color) : null,
        );
      case 'Table':
        final table = AppTable.from(resolveValue(surface, c['data'], scope));
        final columns = c['columns'];
        return AppTableView(
          table: table,
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

  /// [_children], each with its weight (a flex) when it has one.
  List<(Widget, int?)> _weighted(
    BuildContext context,
    Object? list,
    String scope,
    int depth,
  ) {
    (Widget, int?) one(String id, String scope) {
      final weight = surface.components[id]?['weight'];
      return (
        node(context, id, scope, depth + 1),
        weight is num && weight > 0
            ? math.max(1, (weight * 100).round())
            : null,
      );
    }

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
    int depth,
  ) {
    final action = c['action'];
    final event = action is Map<String, Object?> ? action['event'] : null;
    final act = onAction;
    final onPressed = act != null && event is Map<String, Object?>
        ? () => act(id, event, scope)
        : null;
    final childId = c['child'];
    final child = childId is String ? surface.components[childId] : null;
    // A Button's words are its Text child's, drawn as the app's buttons.
    if (child != null && child['component'] == 'Text') {
      final label = string(child['text'], scope);
      if (c['variant'] == 'borderless') {
        return Align(
          alignment: Alignment.centerLeft,
          child: _QuietButton(label: label, onPressed: onPressed),
        );
      }
      return Align(
        alignment: Alignment.centerLeft,
        child: Button(
          label: label,
          primary: c['variant'] == 'primary',
          onPressed: onPressed,
        ),
      );
    }
    return Semantics(
      button: true,
      enabled: onPressed != null,
      child: Pressable(
        onTap: onPressed,
        builder: (context, hovered) => Container(
          padding: const EdgeInsets.all(Space.xs),
          decoration: BoxDecoration(
            color: hovered && onPressed != null
                ? Palette.of(context).hover
                : null,
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: _child(context, childId, scope, depth),
        ),
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
  const _QuietButton({required this.label, required this.onPressed});

  final String label;
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      enabled: onPressed != null,
      label: label,
      excludeSemantics: true,
      child: Pressable(
        onTap: onPressed,
        builder: (context, hovered) => Padding(
          padding: const EdgeInsets.symmetric(vertical: Space.xs),
          child: Text(
            label,
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
              alignment: Alignment.center,
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
              child: Text(
                label,
                style: type.caption.copyWith(
                  color: chosen ? palette.text : palette.textMuted,
                  fontWeight: chosen ? FontWeight.w600 : null,
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
        note: '${widget.appName} gets what you type here.',
        child: field,
      ),
    );
  }
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
