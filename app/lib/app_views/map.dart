part of 'charts.dart';

/// A Map component: a world map as Grafana's geomap draws countries looked
/// up by code, a dot at each country sized (by area) and colored by its
/// value, on the land the app bundles ([_outline], Natural Earth's): no
/// tiles, nothing fetched. Over it its [label]; under it a legend, and in
/// words the rows it can't place (a location that isn't a country's code,
/// such as "Hidden (fewer than 5)", or a country with no value). A pointer
/// over a dot, or a finger's tap on one, shows its country and value.
class AppMap extends StatefulWidget {
  const AppMap({
    super.key,
    required this.table,
    this.location,
    this.value,
    this.label = '',
    this.unit = '',
    this.decimals,
    this.thresholds,
    this.color,
    this.sizeMin = 2,
    this.sizeMax = 15,
    this.legend = true,
    this.height,
  });

  /// Null when the data isn't a table.
  final AppTable? table;

  /// The column of countries, by ISO 3166-1 code (alpha-2 or alpha-3) or
  /// English name; the first text column when null.
  final String? location;

  /// The column drawn; the last numeric column when null.
  final String? value;
  final String label;

  /// One of [chartUnits], or words after the number.
  final String unit;
  final int? decimals;

  /// Colored by these, as a Stat is; by [color] alone when it's set and
  /// these aren't; else green to yellow to red by value.
  final List<StatStep>? thresholds;
  final String? color;

  /// A dot's radius from the least value to the most.
  final double sizeMin;
  final double sizeMax;
  final bool legend;

  /// The map's height; the world's own shape at its width when null or
  /// taller.
  final double? height;

  @override
  State<AppMap> createState() => _AppMapState();
}

/// A country with its value, where its dot goes on the map.
typedef _MapDot = ({
  String code,
  String name,
  double value,
  Offset at,
  double radius,
  Color color,
});

class _AppMapState extends State<AppMap> {
  /// The dot the pointer or a finger is on.
  String? _on;

  /// Put there by a finger or a click: it stays, with a close button.
  bool _pinned = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final caption = type.caption.copyWith(color: palette.textFaint);
    final read = _read();
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
    if (read == null) {
      return labelled(Text("Can't show this map.", style: caption));
    }
    final (placed, others) = read;
    final values = [for (final (_, value) in placed) value];
    final low = values.isEmpty ? 0.0 : values.reduce(math.min);
    final high = values.isEmpty ? 0.0 : values.reduce(math.max);
    String format(double value) =>
        chartValue(value, widget.unit, decimals: widget.decimals);
    final unplaced = [
      for (final (name, value) in others)
        value == null ? '$name: no value' : '$name ${format(value)}',
    ];
    final summary = [
      for (final (country, value) in [
        ...placed,
      ]..sort((a, b) => b.$2.compareTo(a.$2)))
        '${country.name} ${format(value)}',
    ];
    return Semantics(
      container: true,
      label: widget.label.isEmpty ? null : widget.label,
      value: [
        if (summary.isEmpty) 'Nothing on the map' else summary.join(', '),
        if (unplaced.isNotEmpty) 'Not on the map: ${unplaced.join(', ')}',
      ].join('. '),
      child: ExcludeSemantics(
        child: labelled(
          Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            mainAxisSize: MainAxisSize.min,
            spacing: Space.s,
            children: [
              LayoutBuilder(
                builder: (context, constraints) {
                  final width = constraints.maxWidth.isFinite
                      ? constraints.maxWidth
                      : 360.0;
                  // Never taller than the world at this width: on a
                  // phone a tall panel would be bands of empty sea.
                  final height = math.min(
                    widget.height ?? double.infinity,
                    width / _worldAspect,
                  );
                  // The world fitted in the box, its middle in the box's.
                  final world = math.min(width, height * _worldAspect);
                  final frame = Rect.fromCenter(
                    center: Offset(width / 2, height / 2),
                    width: world,
                    height: world / _worldAspect,
                  );
                  final dots = _dots(context, placed, frame, low, high);
                  return SizedBox(
                    width: width,
                    height: height,
                    child: _interactive(context, dots, frame, format),
                  );
                },
              ),
              if (widget.legend && placed.isNotEmpty)
                _MapLegend(
                  low: low,
                  high: high,
                  format: format,
                  thresholds: _steps,
                  color: _fixed(context),
                ),
              if (unplaced.isNotEmpty)
                Text('Not on the map: ${unplaced.join(' · ')}', style: caption),
            ],
          ),
        ),
      ),
    );
  }

  /// Thresholds that color the dots, when they do.
  List<StatStep>? get _steps {
    final steps = widget.thresholds;
    return steps != null && steps.isNotEmpty ? steps : null;
  }

  /// The one color every dot takes, when it's one.
  Color? _fixed(BuildContext context) {
    final color = widget.color;
    return _steps == null && color != null ? statColor(context, color) : null;
  }

  /// The countries placed with their values (summed where a country has
  /// several rows), and the rows not placed: a location that isn't a
  /// country's code, by its own words, or a country with no value.
  (List<(_Country, double)>, List<(String, double?)>)? _read() {
    final table = widget.table;
    if (table == null) return null;
    bool text(int column) {
      for (final row in table.rows) {
        final cell = row[column];
        if (cell != null) return cell is String;
      }
      return false;
    }

    final location = widget.location != null
        ? table.columns.indexOf(widget.location!)
        : [
            for (var i = 0; i < table.columns.length; i++)
              if (text(i)) i,
          ].firstOrNull;
    final value = widget.value != null
        ? table.columns.indexOf(widget.value!)
        : [
            for (var i = table.columns.length - 1; i >= 0; i--)
              if (table.numeric(i)) i,
          ].firstOrNull;
    if (location == null || location < 0 || value == null || value < 0) {
      return null;
    }
    final placed = <String, (_Country, double)>{};
    final others = <String, double?>{};
    for (final row in table.rows) {
      final words = cellText(row[location]).trim();
      final number = cellNumber(row[value]);
      final country = _country(words);
      if (country != null && number != null) {
        final before = placed[country.a2]?.$2 ?? 0;
        placed[country.a2] = (country, before + number);
      } else {
        final name = country?.name ?? (words.isEmpty ? 'No country' : words);
        others[name] = number == null && !others.containsKey(name)
            ? null
            : (others[name] ?? 0) + (number ?? 0);
      }
    }
    return (
      placed.values.toList(),
      [for (final entry in others.entries) (entry.key, entry.value)],
    );
  }

  /// Each country's dot, the biggest first, so smaller ones sit on top.
  List<_MapDot> _dots(
    BuildContext context,
    List<(_Country, double)> placed,
    Rect frame,
    double low,
    double high,
  ) {
    final scale = frame.width / 360;
    final least = widget.sizeMin;
    final most = math.max(widget.sizeMin, widget.sizeMax);
    final fixed = _fixed(context);
    final steps = _steps;
    final dots = [
      for (final (country, value) in placed)
        () {
          final t = high > low ? (value - low) / (high - low) : 1.0;
          // By area: twice the value, twice the dot.
          final radius = math.sqrt(
            least * least + (most * most - least * least) * t.clamp(0, 1),
          );
          return (
            code: country.a2,
            name: country.name,
            value: value,
            at: frame.topLeft + _project(country.lon, country.lat) * scale,
            radius: radius,
            color: steps != null
                ? _stepColor(context, steps, value)
                : fixed ?? _heat(t),
          );
        }(),
    ]..sort((a, b) => b.radius.compareTo(a.radius));
    return dots;
  }

  Widget _interactive(
    BuildContext context,
    List<_MapDot> dots,
    Rect frame,
    String Function(double) format,
  ) {
    _MapDot? nearest(Offset at) {
      _MapDot? best;
      var bestDistance = double.infinity;
      for (final dot in dots) {
        final distance = (dot.at - at).distance;
        // A finger's reach around small dots.
        if (distance <= math.max(dot.radius, 6) + 14 &&
            distance - dot.radius < bestDistance) {
          best = dot;
          bestDistance = distance - dot.radius;
        }
      }
      return best;
    }

    final on = [
      for (final dot in dots)
        if (dot.code == _on) dot,
    ].firstOrNull;
    final dark = MediaQuery.platformBrightnessOf(context) == Brightness.dark;
    return MouseRegion(
      onHover: (event) {
        if (_pinned) return;
        final dot = nearest(event.localPosition);
        if (dot?.code != _on) setState(() => _on = dot?.code);
      },
      onExit: (_) {
        if (!_pinned && _on != null) setState(() => _on = null);
      },
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTapUp: (details) {
          final dot = nearest(details.localPosition);
          setState(() {
            // The same dot again, or away from any: it closes.
            if (dot == null || (_pinned && dot.code == _on)) {
              _on = null;
              _pinned = false;
            } else {
              _on = dot.code;
              _pinned = true;
            }
          });
        },
        child: Stack(
          clipBehavior: Clip.none,
          children: [
            Positioned.fill(
              child: ClipRect(
                child: CustomPaint(
                  painter: _WorldPainter(
                    frame: frame,
                    land: chartText(context)
                        .withValues(alpha: dark ? 0.13 : 0.1),
                    border: Palette.of(context).sidebar,
                    dots: dots,
                    on: _on,
                    ring: chartText(context),
                  ),
                ),
              ),
            ),
            if (on != null)
              Positioned.fill(
                child: CustomSingleChildLayout(
                  delegate: _TooltipPlace(on.at),
                  child: _Tooltip(
                    time: on.name,
                    name:
                        widget.value ??
                        (widget.unit.isEmpty ? 'Value' : widget.unit),
                    color: on.color,
                    value: format(on.value),
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

/// The color [steps] give [value], as a Stat's thresholds do.
Color _stepColor(BuildContext context, List<StatStep> steps, double value) {
  var name = steps.first.color;
  for (final step in steps.skip(1)) {
    final from = step.value;
    if (from != null && value >= from) name = step.color;
  }
  return statColor(context, name) ?? chartText(context);
}

/// Grafana's green-yellow-red by value, [t] from 0 (the least) to 1.
Color _heat(double t) {
  const green = Color(0xFF73BF69);
  const yellow = Color(0xFFFADE2A);
  const red = Color(0xFFF2495C);
  final at = t.clamp(0.0, 1.0);
  return at < 0.5
      ? Color.lerp(green, yellow, at * 2)!
      : Color.lerp(yellow, red, (at - 0.5) * 2)!;
}

/// Under a map: its colors' scale, from the least value to the most (or
/// each threshold's), as Grafana's geomap legend shows a field's.
class _MapLegend extends StatelessWidget {
  const _MapLegend({
    required this.low,
    required this.high,
    required this.format,
    required this.thresholds,
    required this.color,
  });

  final double low;
  final double high;
  final String Function(double) format;
  final List<StatStep>? thresholds;

  /// The one color, when the dots have one: no scale then.
  final Color? color;

  @override
  Widget build(BuildContext context) {
    final style = TypeScale.of(context).caption.copyWith(
      color: chartText(context),
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    final steps = thresholds;
    if (steps != null) {
      return Wrap(
        spacing: Space.m,
        runSpacing: Space.xs,
        children: [
          for (final (i, step) in steps.indexed)
            Row(
              mainAxisSize: MainAxisSize.min,
              spacing: Space.xs + 2,
              children: [
                _swatch(statColor(context, step.color) ?? chartText(context)),
                Text(
                  i + 1 < steps.length
                      ? '${i == 0 ? '' : '${format(step.value ?? 0)}–'}'
                            '< ${format(steps[i + 1].value ?? 0)}'
                      : i == 0
                      ? 'Every value'
                      : '${format(step.value ?? 0)}+',
                  style: style,
                ),
              ],
            ),
        ],
      );
    }
    if (color != null) return const SizedBox.shrink();
    return Row(
      spacing: Space.s,
      children: [
        Text(format(low), style: style),
        Flexible(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 160),
            child: Container(
              height: 6,
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(3),
                gradient: LinearGradient(
                  colors: [_heat(0), _heat(0.5), _heat(1)],
                ),
              ),
            ),
          ),
        ),
        Text(format(high), style: style),
      ],
    );
  }

  Widget _swatch(Color color) => Container(
    width: 10,
    height: 10,
    decoration: BoxDecoration(
      color: color.withValues(alpha: 0.55),
      border: Border.all(color: color),
      shape: BoxShape.circle,
    ),
  );
}

/// The land, then the dots, the one [on] ringed.
class _WorldPainter extends CustomPainter {
  _WorldPainter({
    required this.frame,
    required this.land,
    required this.border,
    required this.dots,
    required this.on,
    required this.ring,
  });

  final Rect frame;
  final Color land;
  final Color border;
  final List<_MapDot> dots;
  final String? on;
  final Color ring;

  @override
  void paint(Canvas canvas, Size size) {
    final scale = frame.width / 360;
    canvas.save();
    canvas.translate(frame.left, frame.top);
    canvas.scale(scale);
    canvas.drawPath(_landPath, Paint()..color = land);
    canvas.drawPath(
      _landPath,
      Paint()
        ..color = border
        ..style = PaintingStyle.stroke
        ..strokeWidth = 0.8 / scale,
    );
    canvas.restore();
    for (final dot in dots) {
      canvas.drawCircle(
        dot.at,
        dot.radius,
        Paint()..color = dot.color.withValues(alpha: 0.55),
      );
      canvas.drawCircle(
        dot.at,
        dot.radius,
        Paint()
          ..color = dot.color
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1,
      );
    }
    for (final dot in dots) {
      if (dot.code != on) continue;
      canvas.drawCircle(
        dot.at,
        dot.radius + 2.5,
        Paint()
          ..color = ring
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1.5,
      );
    }
  }

  @override
  bool shouldRepaint(_WorldPainter old) =>
      old.frame != frame ||
      old.land != land ||
      old.border != border ||
      old.on != on ||
      old.ring != ring ||
      !_sameDots(old.dots, dots);

  static bool _sameDots(List<_MapDot> a, List<_MapDot> b) {
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (a[i] != b[i]) return false;
    }
    return true;
  }
}

// ---- The world ----

/// The map shows from here north to [_south]: where people are, as
/// Grafana's world view does, without the poles Mercator stretches.
const _north = 76.0;
const _south = -56.0;

double _mercator(double lat) =>
    math.log(math.tan(math.pi / 4 + lat * math.pi / 360));

/// Its width over its height.
final _worldAspect = 2 * math.pi / (_mercator(_north) - _mercator(_south));

/// Where [lon], [lat] goes on a world 360 wide (Web Mercator, as
/// Grafana's basemaps are), from the top left.
Offset _project(double lon, double lat) {
  final top = _mercator(_north);
  final span = _mercator(_north) - _mercator(_south);
  final y = (top - _mercator(lat.clamp(-85, 85))) / span * (360 / _worldAspect);
  return Offset(lon + 180, y);
}

/// A country by code.
typedef _Country = ({
  String a2,
  String a3,
  String name,
  double lon,
  double lat,
});

final Map<String, _Country> _byCode = () {
  final out = <String, _Country>{};
  for (final (a2, a3, name, lon, lat) in _countries) {
    final country = (a2: a2, a3: a3, name: name, lon: lon, lat: lat);
    out[a2] = country;
    out[a3] = country;
  }
  return out;
}();

/// The country [words] name: its ISO 3166-1 alpha-2 or alpha-3 code, or
/// its English name (Natural Earth's, or as GeoIP databases spell it:
/// "Türkiye", "DR Congo"), in any case, as Grafana's gazetteer looks up
/// countries.
_Country? _country(String words) {
  final code = words.trim();
  if (code.length == 2 || code.length == 3) {
    final country = _byCode[code.toUpperCase()];
    if (country != null) return country;
  }
  final a2 = _countryNames[_fold(words)];
  return a2 == null ? null : _byCode[a2];
}

/// [text] as country names are compared: lower case, without accents, one
/// kind of apostrophe, single spaces.
String _fold(String text) {
  final out = StringBuffer();
  var space = false;
  for (final rune in text.trim().toLowerCase().runes) {
    final char = String.fromCharCode(rune);
    if (char.trim().isEmpty) {
      space = true;
      continue;
    }
    if (space && out.isNotEmpty) out.write(' ');
    space = false;
    out.write(char == '\u2019' ? "'" : _plainLetters[char] ?? char);
  }
  return out.toString();
}

/// The English name of the country [code] names (a code or a name), for
/// tests and others.
String? countryName(String code) => _country(code)?.name;

/// The land, projected on a world 360 wide.
final Path _landPath = () {
  final bytes = base64Decode(_outline);
  final values = Int16List.view(
    bytes.buffer,
    bytes.offsetInBytes,
    bytes.length ~/ 2,
  );
  final path = Path();
  var at = 0;
  while (at < values.length) {
    final count = values[at++];
    for (var i = 0; i < count && at + 1 < values.length; i++) {
      final point = _project(values[at] / 10, values[at + 1] / 10);
      at += 2;
      i == 0
          ? path.moveTo(point.dx, point.dy)
          : path.lineTo(point.dx, point.dy);
    }
    path.close();
  }
  return path;
}();
