import 'dart:convert';
import 'dart:math' as math;
import 'dart:ui';

/// An agent's body: one of six plain geometric shapes, all inside the same
/// box, in the backend's catalog order.
enum CharacterShape { circle, square, triangle, diamond, pentagon, hexagon }

/// What the agent is doing, as its eyes show it.
enum Mood { idle, thinking, working, news, failed, noModel, asleep }

/// Who a character is: its shape and color (backend-owned identity) and the
/// seed that keeps its blinks and glances out of step with other agents'.
/// Every agent has the same eyes.
class CharacterSpec {
  const CharacterSpec({
    required this.shape,
    required this.color,
    required this.seed,
  });

  final CharacterShape shape;

  /// The agent's color, unchanged: whose agent it is, in every mood.
  final Color color;
  final int seed;

  @override
  bool operator ==(Object other) =>
      other is CharacterSpec &&
      other.shape == shape &&
      other.color == color &&
      other.seed == seed;

  @override
  int get hashCode => Object.hash(shape, color, seed);
}

/// The box's padding: every shape fits inside [inset, 1 - inset]. The
/// polygons reach nearly to it; the circle, which looks bigger at the same
/// width, stays 2% further in.
const characterInset = 0.04;

/// How much a polygon's corners are softened, in box units: enough to look
/// finished, little enough that each reads as the polygon it is.
const cornerRadius = 0.05;

/// Samples around a body's outline, at even angles about the box's center.
const profileSamples = 64;

/// Whether the point ([x], [y]), relative to the box's center in box units
/// (y down), is inside [shape] as it's drawn.
bool insideShape(CharacterShape shape, double x, double y) =>
    bodyPath(shape, 1).contains(Offset(0.5 + x, 0.5 + y));

/// A polygon's corners in box units, clockwise on screen (y down).
List<Offset>? _corners(CharacterShape shape) {
  List<Offset> regular(int sides, double radius, double cy, double turn) => [
    for (var i = 0; i < sides; i++)
      Offset(
        0.5 + radius * math.cos(turn + 2 * math.pi * i / sides),
        cy + radius * math.sin(turn + 2 * math.pi * i / sides),
      ),
  ];
  switch (shape) {
    case CharacterShape.circle:
      return null;
    case CharacterShape.square:
      return const [
        Offset(0.13, 0.13),
        Offset(0.87, 0.13),
        Offset(0.87, 0.87),
        Offset(0.13, 0.87),
      ];
    case CharacterShape.triangle:
      // Pointing up, wide enough for the eyes low in it.
      return const [Offset(0.5, 0.05), Offset(0.96, 0.9), Offset(0.04, 0.9)];
    case CharacterShape.diamond:
      return const [
        Offset(0.5, 0.04),
        Offset(0.96, 0.5),
        Offset(0.5, 0.96),
        Offset(0.04, 0.5),
      ];
    case CharacterShape.pentagon:
      // Point up, its middle a little low so it sits square.
      return regular(5, 0.46, 0.53, -math.pi / 2);
    case CharacterShape.hexagon:
      // Flat top and bottom.
      return regular(6, 0.46, 0.5, 0);
  }
}

final _profiles = <CharacterShape, List<double>>{};

/// The distance from the box's center to [shape]'s outline at each of
/// [profileSamples] angles (starting straight up, clockwise), in box units.
/// Every shape is star-shaped about the center, so each ray crosses its
/// outline once.
List<double> shapeProfile(CharacterShape shape) =>
    _profiles[shape] ??= List.unmodifiable([
      for (var i = 0; i < profileSamples; i++) _edge(shape, _angle(i)),
    ]);

double _angle(int i) => -math.pi / 2 + 2 * math.pi * i / profileSamples;

double _edge(CharacterShape shape, double angle) {
  final dx = math.cos(angle), dy = math.sin(angle);
  var inside = 0.0, outside = 0.5;
  for (var step = 0; step < 30; step++) {
    final middle = (inside + outside) / 2;
    if (insideShape(shape, dx * middle, dy * middle)) {
      inside = middle;
    } else {
      outside = middle;
    }
  }
  return inside;
}

final _bodies = <(CharacterShape, double), Path>{};

/// [shape]'s outline in a [size]-wide box, cached: a circle, or a polygon
/// with its corners softened by [cornerRadius].
Path bodyPath(CharacterShape shape, double size) =>
    _bodies[(shape, size)] ??= _exact(shape, size);

Path _exact(CharacterShape shape, double size) {
  final corners = _corners(shape);
  if (corners == null) {
    return Path()..addOval(
      Rect.fromCircle(center: Offset(size / 2, size / 2), radius: size * 0.44),
    );
  }
  final path = Path();
  final n = corners.length;
  final radius = cornerRadius * size;
  for (var i = 0; i < n; i++) {
    final corner = corners[i] * size;
    final before = corners[(i - 1 + n) % n] * size;
    final after = corners[(i + 1) % n] * size;
    final a = before - corner, b = after - corner;
    final ua = a / a.distance, ub = b / b.distance;
    final angle = math.acos((ua.dx * ub.dx + ua.dy * ub.dy).clamp(-1.0, 1.0));
    // Where the corner's arc meets each side.
    final reach = radius / math.tan(angle / 2);
    final start = corner + ua * reach, end = corner + ub * reach;
    i == 0 ? path.moveTo(start.dx, start.dy) : path.lineTo(start.dx, start.dy);
    path.arcToPoint(end, radius: Radius.circular(radius));
  }
  return path..close();
}

/// The outline between two shapes, part way ([t]) from [from] to [to]: for
/// the brief morph when the person picks another shape, through their
/// outlines sampled at even angles and joined smoothly. Not cached.
Path morphPath(CharacterShape from, CharacterShape to, double t, double size) {
  final a = shapeProfile(from), b = shapeProfile(to);
  return _smooth(
    _points([
      for (var i = 0; i < profileSamples; i++) a[i] + (b[i] - a[i]) * t,
    ], size),
  );
}

List<Offset> _points(List<double> radii, double size) => [
  for (var i = 0; i < radii.length; i++)
    Offset(
      size * (0.5 + radii[i] * math.cos(_angle(i))),
      size * (0.5 + radii[i] * math.sin(_angle(i))),
    ),
];

Path _smooth(List<Offset> points) {
  final n = points.length;
  final path = Path()..moveTo(points[0].dx, points[0].dy);
  for (var i = 0; i < n; i++) {
    final p0 = points[(i - 1 + n) % n];
    final p1 = points[i];
    final p2 = points[(i + 1) % n];
    final p3 = points[(i + 2) % n];
    final c1 = p1 + (p2 - p0) / 6;
    final c2 = p2 - (p3 - p1) / 6;
    path.cubicTo(c1.dx, c1.dy, c2.dx, c2.dy, p2.dx, p2.dy);
  }
  return path..close();
}

/// Where the eyes sit in a body and how big they are, in box units: white
/// capsules, the left one first.
class EyeLayout {
  const EyeLayout({
    required this.left,
    required this.right,
    required this.width,
    required this.height,
  });

  final Offset left;
  final Offset right;
  final double width;
  final double height;

  /// The same eyes made a little bigger, for tiny characters.
  EyeLayout scaled(double factor) => EyeLayout(
    left: left,
    right: right,
    width: width * factor,
    height: height * factor,
  );
}

/// The eyes, the same on every agent: upright capsules a little above the
/// middle, set lower and a touch smaller where a shape narrows toward its
/// top (the triangle, the diamond, the pentagon).
EyeLayout eyeLayout(CharacterShape shape) => switch (shape) {
  CharacterShape.circle ||
  CharacterShape.square ||
  CharacterShape.hexagon => const EyeLayout(
    left: Offset(0.375, 0.47),
    right: Offset(0.625, 0.47),
    width: 0.13,
    height: 0.27,
  ),
  CharacterShape.pentagon => const EyeLayout(
    left: Offset(0.38, 0.52),
    right: Offset(0.62, 0.52),
    width: 0.125,
    height: 0.26,
  ),
  CharacterShape.diamond => const EyeLayout(
    left: Offset(0.395, 0.5),
    right: Offset(0.605, 0.5),
    width: 0.115,
    height: 0.24,
  ),
  CharacterShape.triangle => const EyeLayout(
    left: Offset(0.4, 0.635),
    right: Offset(0.6, 0.635),
    width: 0.11,
    height: 0.22,
  ),
};

/// How far the eyes may look (gaze, in box units, either way), so they stay
/// inside every body at every size: checked by the tests for each shape.
const gazeLimit = 0.05;

/// Eyes grow this much at 18px and under, so they still read.
const smallEyeScale = 1.15;

/// 32-bit FNV-1a of [text]'s UTF-8 bytes.
int fnv1a(String text) {
  var hash = 0x811c9dc5;
  for (final byte in utf8.encode(text)) {
    hash ^= byte;
    hash = (hash * 0x01000193) & 0xffffffff;
  }
  return hash;
}
