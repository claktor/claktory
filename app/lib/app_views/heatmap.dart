part of 'charts.dart';

/// A Chart of kind heatmap: a grid of [x]'s values (its columns) by
/// [seriesBy]'s (its rows), each in the order the rows first have them,
/// every cell colored by its [y] from faint to full orange, as Grafana's
/// heatmap shades a bucket by its count. A cell with no value stays empty.
/// Under it a legend of the scale; a pointer over a cell, or a finger's
/// tap on one, shows its row, column and value.
class AppHeatmap extends StatefulWidget {
  const AppHeatmap({
    super.key,
    required this.table,
    this.x,
    this.y = const [],
    this.seriesBy,
    this.label = '',
    this.unit = '',
    this.decimals,
    this.legend = true,
    this.height = 200,
  });

  /// Null when the data isn't a table.
  final AppTable? table;

  /// The columns' column; the first that isn't [seriesBy] or the value
  /// when null.
  final String? x;

  /// The value's column, first of these; the last numeric column when
  /// empty.
  final List<String> y;

  /// The rows' column; one row when null.
  final String? seriesBy;
  final String label;
  final String unit;
  final int? decimals;
  final bool legend;
  final double height;

  @override
  State<AppHeatmap> createState() => _AppHeatmapState();
}

/// At most this many columns and rows in a heatmap.
const maxHeatmapCells = 200;

class _AppHeatmapState extends State<AppHeatmap> {
  /// The cell the pointer or a finger is on: (row, column).
  (int, int)? _on;
  bool _pinned = false;

  /// The grid: its rows' and columns' names and each cell's value.
  ({List<String> rows, List<String> columns, Map<(int, int), double> cells})?
  _grid() {
    final table = widget.table;
    if (table == null) return null;
    final columns = table.columns;
    int? named(String? name) {
      if (name == null) return null;
      final at = columns.indexOf(name);
      return at < 0 ? -1 : at;
    }

    final rowAt = named(widget.seriesBy);
    final valueAt = widget.y.isNotEmpty
        ? named(widget.y.first)
        : [
            for (var i = columns.length - 1; i >= 0; i--)
              if (table.numeric(i) && i != rowAt) i,
          ].firstOrNull;
    final columnAt =
        named(widget.x) ??
        [
          for (var i = 0; i < columns.length; i++)
            if (i != rowAt && i != valueAt) i,
        ].firstOrNull;
    if ((rowAt ?? 0) < 0 ||
        valueAt == null ||
        valueAt < 0 ||
        columnAt == null ||
        columnAt < 0) {
      return null;
    }
    final rows = <String>[];
    final names = <String>[];
    final cells = <(int, int), double>{};
    for (final row in table.rows) {
      final rowName = rowAt == null ? '' : cellText(row[rowAt]);
      final columnName = cellText(row[columnAt]);
      var r = rows.indexOf(rowName);
      if (r < 0) {
        if (rows.length >= maxHeatmapCells) continue;
        rows.add(rowName);
        r = rows.length - 1;
      }
      var c = names.indexOf(columnName);
      if (c < 0) {
        if (names.length >= maxHeatmapCells) continue;
        names.add(columnName);
        c = names.length - 1;
      }
      final value = cellNumber(row[valueAt]);
      if (value != null) cells[(r, c)] = (cells[(r, c)] ?? 0) + value;
    }
    return (rows: rows, columns: names, cells: cells);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final caption = type.caption.copyWith(color: palette.textFaint);
    final grid = _grid();
    Widget labelled(Widget body) => widget.label.isEmpty
        ? body
        : Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            mainAxisSize: MainAxisSize.min,
            spacing: Space.xs,
            children: [
              Text(
                widget.label,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: panelTitle(context),
              ),
              body,
            ],
          );
    if (grid == null) {
      return labelled(Text("Can't show this chart.", style: caption));
    }
    if (grid.cells.isEmpty) {
      return labelled(Text('Nothing to show.', style: caption));
    }
    final values = grid.cells.values;
    final low = values.reduce(math.min);
    final high = values.reduce(math.max);
    String format(double value) =>
        chartValue(value, widget.unit, decimals: widget.decimals);
    final orange = chartColor(context, 'orange') ?? palette.signal;
    final axis = TypeScale.of(context).caption.copyWith(
      fontSize: 12,
      height: 1.2,
      color: chartText(context),
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    final hottest = grid.cells.entries.reduce(
      (a, b) => b.value > a.value ? b : a,
    );
    final summary =
        '${grid.rows.length} by ${grid.columns.length}, from ${format(low)} '
        'to ${format(high)}; the most, ${format(hottest.value)}, at '
        '${_cellName(grid.rows, grid.columns, hottest.key)}';
    return Semantics(
      container: true,
      label: widget.label.isEmpty ? null : widget.label,
      value: summary,
      child: ExcludeSemantics(
        child: labelled(
          Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            mainAxisSize: MainAxisSize.min,
            spacing: Space.s,
            children: [
              SizedBox(
                height: widget.height,
                child: MediaQuery.withClampedTextScaling(
                  maxScaleFactor: maxAxisTextScale,
                  child: LayoutBuilder(
                    builder: (context, constraints) => _plot(
                      context,
                      constraints.biggest,
                      grid,
                      low,
                      high,
                      orange,
                      axis,
                      format,
                    ),
                  ),
                ),
              ),
              if (widget.legend)
                Row(
                  spacing: Space.s,
                  children: [
                    Text(format(low), style: axis),
                    Flexible(
                      child: ConstrainedBox(
                        constraints: const BoxConstraints(maxWidth: 160),
                        child: Container(
                          height: 6,
                          decoration: BoxDecoration(
                            borderRadius: BorderRadius.circular(3),
                            gradient: LinearGradient(
                              colors: [
                                _shade(orange, 0),
                                _shade(orange, 0.5),
                                _shade(orange, 1),
                              ],
                            ),
                          ),
                        ),
                      ),
                    ),
                    Text(format(high), style: axis),
                  ],
                ),
            ],
          ),
        ),
      ),
    );
  }

  static String _cellName(
    List<String> rows,
    List<String> columns,
    (int, int) cell,
  ) => [
    if (rows[cell.$1].isNotEmpty) rows[cell.$1],
    columns[cell.$2],
  ].join(' · ');

  Widget _plot(
    BuildContext context,
    Size size,
    ({List<String> rows, List<String> columns, Map<(int, int), double> cells})
    grid,
    double low,
    double high,
    Color orange,
    TextStyle axis,
    String Function(double) format,
  ) {
    final scaler = MediaQuery.textScalerOf(context);
    double widthOf(String text) {
      final painter = TextPainter(
        text: TextSpan(text: text, style: axis),
        textDirection: TextDirection.ltr,
        textScaler: scaler,
      )..layout();
      final width = painter.width;
      painter.dispose();
      return width;
    }

    final lineHeight = scaler.scale(axis.fontSize!) * axis.height!;
    // The rows' names at the left, as wide as the widest; the columns'
    // under the grid, as many as fit apart.
    final left = grid.rows.every((name) => name.isEmpty)
        ? 0.0
        : grid.rows.map(widthOf).reduce(math.max) + Space.s;
    final bottom = lineHeight + Space.xs;
    final plot = Rect.fromLTWH(
      left,
      0,
      math.max(1, size.width - left),
      math.max(1, size.height - bottom),
    );
    final cellWidth = plot.width / grid.columns.length;
    final cellHeight = plot.height / grid.rows.length;
    final widest = grid.columns.map(widthOf).reduce(math.max) + Space.s;
    final every = math.max(1, (widest / cellWidth).ceil());
    (int, int)? cellAt(Offset at) {
      if (!plot.contains(at)) return null;
      final r = ((at.dy - plot.top) / cellHeight).floor();
      final c = ((at.dx - plot.left) / cellWidth).floor();
      if (r < 0 || c < 0 || r >= grid.rows.length || c >= grid.columns.length) {
        return null;
      }
      return (r, c);
    }

    final on = _on;
    final onValue = on == null ? null : grid.cells[on];
    final onCenter = on == null
        ? null
        : Offset(
            plot.left + (on.$2 + 0.5) * cellWidth,
            plot.top + (on.$1 + 0.5) * cellHeight,
          );
    return MouseRegion(
      onHover: (event) {
        if (_pinned) return;
        final cell = cellAt(event.localPosition);
        if (cell != _on) setState(() => _on = cell);
      },
      onExit: (_) {
        if (!_pinned && _on != null) setState(() => _on = null);
      },
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTapUp: (details) {
          final cell = cellAt(details.localPosition);
          setState(() {
            if (cell == null || (_pinned && cell == _on)) {
              _on = null;
              _pinned = false;
            } else {
              _on = cell;
              _pinned = true;
            }
          });
        },
        child: Stack(
          clipBehavior: Clip.none,
          children: [
            Positioned.fill(
              child: CustomPaint(
                painter: _HeatmapPainter(
                  plot: plot,
                  rows: grid.rows.length,
                  columns: grid.columns.length,
                  cells: {
                    for (final MapEntry(:key, :value) in grid.cells.entries)
                      key: _shade(
                        orange,
                        high > low ? (value - low) / (high - low) : 1,
                      ),
                  },
                  on: on,
                  ring: chartText(context),
                ),
              ),
            ),
            for (final (r, name) in grid.rows.indexed)
              if (name.isNotEmpty)
                Positioned(
                  left: 0,
                  width: left - Space.s,
                  top: plot.top + r * cellHeight,
                  height: cellHeight,
                  child: Align(
                    alignment: Alignment.centerRight,
                    child: Text(name, maxLines: 1, style: axis),
                  ),
                ),
            for (var c = 0; c < grid.columns.length; c += every)
              Positioned(
                left: plot.left + c * cellWidth - widest / 2 + cellWidth / 2,
                width: widest,
                top: plot.bottom + Space.xs,
                child: Text(
                  grid.columns[c],
                  maxLines: 1,
                  textAlign: TextAlign.center,
                  style: axis,
                ),
              ),
            if (on != null && onCenter != null)
              Positioned.fill(
                child: CustomSingleChildLayout(
                  delegate: _TooltipPlace(onCenter),
                  child: _Tooltip(
                    time: _cellName(grid.rows, grid.columns, on),
                    name: widget.y.isNotEmpty
                        ? widget.y.first
                        : (widget.unit.isEmpty ? 'Value' : widget.unit),
                    color: onValue == null
                        ? chartText(context)
                        : _shade(
                            orange,
                            high > low ? (onValue - low) / (high - low) : 1,
                          ),
                    value: onValue == null ? 'No value' : format(onValue),
                    onClose: _pinned
                        ? () => setState(() {
                            _on = null;
                            _pinned = false;
                          })
                        : null,
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

/// [color] for [t] from 0 (the least) to 1 (the most): faint to full, on
/// Grafana's square-root scale, so the low end still tells apart.
Color _shade(Color color, double t) =>
    color.withValues(alpha: 0.12 + 0.88 * math.sqrt(t.clamp(0.0, 1.0)));

class _HeatmapPainter extends CustomPainter {
  _HeatmapPainter({
    required this.plot,
    required this.rows,
    required this.columns,
    required this.cells,
    required this.on,
    required this.ring,
  });

  final Rect plot;
  final int rows;
  final int columns;
  final Map<(int, int), Color> cells;
  final (int, int)? on;
  final Color ring;

  @override
  void paint(Canvas canvas, Size size) {
    final width = plot.width / columns;
    final height = plot.height / rows;
    // A pixel between cells, as Grafana's gap, where they're big enough.
    final gap = width > 4 && height > 4 ? 1.0 : 0.0;
    Rect cell(int r, int c) => Rect.fromLTWH(
      plot.left + c * width + gap / 2,
      plot.top + r * height + gap / 2,
      math.max(0, width - gap),
      math.max(0, height - gap),
    );
    for (final MapEntry(key: (r, c), :value) in cells.entries) {
      canvas.drawRect(cell(r, c), Paint()..color = value);
    }
    final on = this.on;
    if (on != null) {
      canvas.drawRect(
        cell(on.$1, on.$2),
        Paint()
          ..color = ring
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1.5,
      );
    }
  }

  @override
  bool shouldRepaint(_HeatmapPainter old) =>
      old.plot != plot ||
      old.rows != rows ||
      old.columns != columns ||
      old.on != on ||
      old.ring != ring ||
      !mapEquals(old.cells, cells);
}
