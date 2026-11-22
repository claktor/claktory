import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'avatar.dart';
import 'backend.dart' show fireAndLog;
import 'canvas.dart';
import 'characters/character.dart' show AgentCharacter, identityOf;
import 'controls.dart';
import 'identity.dart';
import 'map_card.dart';
import 'map_layout.dart';
import 'mobile/drawer.dart' show largeText;
import 'mobile/header.dart';
import 'mobile/metrics.dart';
import 'mobile/navigation.dart';
import 'mobile/input.dart' show InputKind;
import 'mobile/presentation.dart' show showPanel;
import 'pane_chrome.dart';
import 'pipes.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart';

/// The color of a [MapLayer]'s lines. Not agent colors, which say whose
/// something is, and not signal, which marks selection: hues of their
/// own, a little lighter in the dark.
Color mapLayerColor(MapLayer layer, Palette palette) {
  final dark = palette.canvas.computeLuminance() < 0.5;
  return switch (layer) {
    MapLayer.messages =>
      dark ? const Color(0xFF5AA6E8) : const Color(0xFF1D6FC2),
    MapLayer.leads => dark ? const Color(0xFFE6809F) : const Color(0xFFB23A63),
    MapLayer.secrets =>
      dark ? const Color(0xFFE2A93E) : const Color(0xFFA66A00),
    MapLayer.hierarchy => palette.textFaint,
    MapLayer.data => dark ? const Color(0xFF52B985) : const Color(0xFF16804B),
    MapLayer.apps => dark ? const Color(0xFFB394F0) : const Color(0xFF7248C4),
  };
}

/// What each layer is called in the key.
const mapLayerNames = {
  MapLayer.messages: 'Messages',
  MapLayer.leads: 'Leads',
  MapLayer.secrets: 'Secrets',
  MapLayer.hierarchy: 'Under other agents',
  MapLayer.data: "Claktory's records",
  MapLayer.apps: 'Apps',
};

/// The Map: how this Claktory is arranged, drawn like a circuit board.
/// Rivet and the other admins along the top, then the workspace and each
/// project as a board with its folders and agents, and the vault along the
/// bottom; lines show who can message whom, who has which secret and who
/// reads Claktory's records. A project's pipes to other Claktories are
/// portal pads at its top right, each with a line leaving the board; a click
/// opens the pipe. Hover an agent to light up its lines and,
/// after a moment, see its card (every extension it has, where it works,
/// whom it messages, its secrets); click it to open its conversation. On a
/// phone, with no pointer, a tap lights it up and shows its card, with Open
/// conversation.
class MapView extends StatefulWidget {
  const MapView({super.key});

  @override
  State<MapView> createState() => _MapViewState();
}

class _MapViewState extends State<MapView> {
  final _canvas = CanvasController();
  final _hidden = <MapLayer>{};
  String? _hover;
  String? _pinned;

  /// The agent whose card shows on hover, once the pointer has rested.
  String? _card;
  Timer? _cardWait;

  ClaktoryMap? _laidOut;
  Map<String, int>? _order;
  var _layout = MapLayout.empty;

  @override
  void dispose() {
    _cardWait?.cancel();
    _canvas.dispose();
    super.dispose();
  }

  /// Lays the map out again only when it changed: the workspace comes
  /// again with every agent's every step.
  MapLayout _layoutFor(Workspace workspace) {
    // A pipe's liaison sits on its pipe's pad, not as a chip of its own.
    final liaisons = pipeLiaisons(workspace);
    final order = sidebarOrder(workspace)
      ..removeWhere(
        (id, _) => liaisons.values.any((agent) => id == 'agent:${agent.id}'),
      );
    if (_laidOut != workspace.map || !_sameOrder(order)) {
      _laidOut = workspace.map;
      _order = order;
      _layout = layoutMap(
        mapWithout(workspace.map, {
          for (final agent in liaisons.values) 'agent:${agent.id}',
        }),
        order: order,
        you: true,
      );
    }
    return _layout;
  }

  bool _sameOrder(Map<String, int> order) {
    final old = _order;
    if (old == null || old.length != order.length) return false;
    for (final MapEntry(:key, :value) in order.entries) {
      if (old[key] != value) return false;
    }
    return true;
  }

  void _hovered(String? id) {
    _cardWait?.cancel();
    setState(() {
      _hover = id;
      if (id != _card) _card = null;
    });
    if (id == null || !id.startsWith('agent:')) return;
    // Waits a moment, as hover cards do, so it doesn't flash by.
    _cardWait = Timer(const Duration(milliseconds: 300), () {
      if (mounted && _hover == id) setState(() => _card = id);
    });
  }

  void _open(MapNode node) {
    final agent = node.kind == MapNodeKind.MAP_NODE_KIND_AGENT;
    if (!agent || isPhoneLayout(context)) {
      // Anything else lights up its lines, and off again; on a phone an
      // agent too, with its card.
      setState(() => _pinned = _pinned == node.id ? null : node.id);
      return;
    }
    _openConversation(node);
  }

  void _openConversation(MapNode node) {
    final backend = AppScope.of(context);
    AppScope.closePage(context);
    fireAndLog(backend.selectAgent(node.refId));
    // On a phone, its conversation is pushed over the list.
    PhoneNavigation.of(context)?.value = true;
  }

  /// [chip]'s card: on a Mac beside it, following the canvas as it moves,
  /// and never in the pointer's way; on a phone along the bottom, with
  /// Close and Open conversation.
  Widget _cardFor(Workspace workspace, MapChip chip, {required bool phone}) {
    final details = mapAgentDetails(workspace, chip.node);
    if (phone) {
      return Positioned(
        left: Space.s,
        right: Space.s,
        bottom: Space.s,
        child: Center(
          child: ConstrainedBox(
            // At most about half the screen: the map stays in view above
            // it, and its details scroll.
            constraints: BoxConstraints(
              maxWidth: 420,
              maxHeight:
                  MediaQuery.sizeOf(context).height *
                  (largeText(context) ? 0.85 : 0.5),
            ),
            child: MapAgentCard(
              width: null,
              details: details,
              actions: [
                Button(
                  label: 'Close',
                  onPressed: () => setState(() => _pinned = null),
                ),
                Button(
                  label: 'Open conversation',
                  primary: true,
                  onPressed: () => _openConversation(chip.node),
                ),
              ],
            ),
          ),
        ),
      );
    }
    return Positioned.fill(
      child: IgnorePointer(
        child: ListenableBuilder(
          listenable: _canvas,
          builder: (context, _) => CustomSingleChildLayout(
            delegate: MapCardPlacement(_canvas.toView(chip.rect)),
            child: TweenAnimationBuilder<double>(
              tween: Tween(begin: 0, end: 1),
              duration: Motion.fast,
              curve: Curves.easeOut,
              builder: (context, t, child) => Opacity(opacity: t, child: child),
              child: MapAgentCard(details: details),
            ),
          ),
        ),
      ),
    );
  }

  /// Lights up [id] (an agent's chip shows its card) and brings it into
  /// view: from the list of what's on the map.
  void _pick(String id) {
    PhoneRoutes.of(context)?.pop<void>();
    setState(() => _pinned = id);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      final chip = _layout.chips.where((chip) => chip.id == id).firstOrNull;
      if (chip != null) _canvas.show(chip.rect.inflate(120));
    });
  }

  /// Everything on the map as a list, to search: for VoiceOver, large
  /// text, or finding one agent among many.
  void _openList() => PhoneRoutes.of(context)?.push<void>(
    PhoneRoute(
      id: 'map:list',
      builder: (context) => _MapList(onPick: _pick),
    ),
  );

  /// Which kinds of line show, and what the badges mean, in a sheet.
  void _openLines(MapLayout layout) {
    final layers = [
      for (final layer in MapLayer.values)
        if (layout.wires.any((wire) => wire.layer == layer)) layer,
    ];
    final badges = <String, MapBadge>{};
    for (final chip in layout.chips) {
      for (final badge in chip.node.badges) {
        badges.putIfAbsent(badge.key, () => badge);
      }
    }
    showPanel(
      context,
      title: 'Lines',
      builder: (context, close) => StatefulBuilder(
        builder: (context, setSheet) => _LinesPanel(
          layers: layers,
          badges: badges.values.toList(),
          hidden: _hidden,
          onToggle: (layer) {
            setState(() {
              if (!_hidden.remove(layer)) _hidden.add(layer);
            });
            setSheet(() {});
          },
        ),
      ),
    );
  }

  /// The Map on a phone: Back, Search and Lines in its header, the board
  /// filling the rest at its own type size (a pinch reads it closer), and
  /// Fit at its foot, or in its place the card of the agent tapped.
  Widget _phoneMap(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    void close() => AppScope.closePage(context);
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): close},
      child: ListenableBuilder(
        listenable: backend.workspace,
        builder: (context, _) {
          final workspace = backend.workspace.workspace;
          final empty = !workspace.hasMap() || workspace.map.nodes.isEmpty;
          final layout = empty ? null : _layoutFor(workspace);
          Widget glyph(String icon, Color color) =>
              ExtensionGlyph(icon: icon, name: '', color: color, size: 20);
          return Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              PhoneHeader(
                leading: HeaderButton.back(onTap: close),
                title: Text(
                  'Map',
                  style: PhoneType.emphasis.copyWith(color: palette.text),
                ),
                trailing: [
                  if (layout != null) ...[
                    HeaderButton(
                      label: 'Search the map',
                      glyph: (color) => glyph('search', color),
                      onTap: _openList,
                    ),
                    HeaderButton(
                      label: 'Lines',
                      active: _hidden.isNotEmpty,
                      glyph: (color) => glyph('list-filter', color),
                      onTap: () => _openLines(layout),
                    ),
                  ],
                ],
              ),
              Expanded(
                child: ColoredBox(
                  color: palette.canvas,
                  child: layout == null
                      ? Center(
                          child: Padding(
                            padding: const EdgeInsets.all(Space.xl),
                            child: Text(
                              'Update Claktory to see its map.',
                              textAlign: TextAlign.center,
                              style: PhoneType.body.copyWith(
                                color: palette.textMuted,
                              ),
                            ),
                          ),
                        )
                      : _phoneBoard(workspace, layout),
                ),
              ),
            ],
          );
        },
      ),
    );
  }

  Widget _phoneBoard(Workspace workspace, MapLayout layout) {
    final palette = Palette.of(context);
    final pinned = _pinned;
    final cardChip = pinned == null || !pinned.startsWith('agent:')
        ? null
        : layout.chips.where((chip) => chip.id == pinned).firstOrNull;
    return Stack(
      children: [
        Positioned.fill(
          // The board is drawn to its own measure: the system's text size
          // would burst its boxes. A pinch reads it closer, and the list
          // (Search) reads at any size.
          child: MediaQuery.withNoTextScaling(
            child: ZoomCanvas(
              controller: _canvas,
              size: layout.size,
              background: (transform) =>
                  _GridPainter(transform, palette.hairline),
              child: _Board(
                layout: layout,
                pads: portalPads(
                  layout,
                  workspace.pipes,
                  liaisons: pipeLiaisons(workspace),
                ),
                person: workspace.person,
                hidden: _hidden,
                focus: pinned,
                phone: true,
                onHover: (_) {},
                onTap: _open,
              ),
            ),
          ),
        ),
        if (cardChip != null)
          _cardFor(workspace, cardChip, phone: true)
        else
          Positioned(
            right: Touch.gutter,
            bottom: Touch.gutter,
            child: _FitButton(controller: _canvas),
          ),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final inPane = InPane.of(context);
    final phone = isPhoneLayout(context);
    void close() => AppScope.closePage(context);
    if (phone && !inPane) return _phoneMap(context);
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): close},
      child: FocusScope(
        autofocus: !inPane,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (!inPane)
              WindowDrag(
                child: Container(
                  height: titleBarHeight,
                  padding: const EdgeInsets.only(
                    left: Space.xl,
                    right: Space.m,
                  ),
                  decoration: BoxDecoration(
                    border: Border(bottom: BorderSide(color: palette.hairline)),
                  ),
                  child: Row(
                    children: [
                      Text(
                        'Map',
                        style: TextStyles.label.copyWith(
                          color: palette.text,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                      const Spacer(),
                      Button(label: 'Done', onPressed: close),
                    ],
                  ),
                ),
              ),
            Expanded(
              child: ColoredBox(
                color: palette.canvas,
                child: ListenableBuilder(
                  listenable: backend.workspace,
                  builder: (context, _) {
                    final workspace = backend.workspace.workspace;
                    if (!workspace.hasMap() || workspace.map.nodes.isEmpty) {
                      return Center(
                        child: Padding(
                          padding: const EdgeInsets.all(Space.xl),
                          child: Text(
                            'Update Claktory to see its map.',
                            textAlign: TextAlign.center,
                            style: TextStyles.body.copyWith(
                              color: palette.textMuted,
                            ),
                          ),
                        ),
                      );
                    }
                    final layout = _layoutFor(workspace);
                    final focus = _hover ?? _pinned;
                    // On a phone, the tapped agent's card; on a Mac, the
                    // hovered one's.
                    final cardId = phone ? _pinned : _card;
                    final cardChip =
                        cardId == null || !cardId.startsWith('agent:')
                        ? null
                        : layout.chips
                              .where((chip) => chip.id == cardId)
                              .firstOrNull;
                    return Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        _Key(
                          layout: layout,
                          hidden: _hidden,
                          phone: phone,
                          canvas: _canvas,
                          onToggle: (layer) => setState(() {
                            if (!_hidden.remove(layer)) _hidden.add(layer);
                          }),
                        ),
                        Expanded(
                          child: Stack(
                            children: [
                              Positioned.fill(
                                child: ZoomCanvas(
                                  controller: _canvas,
                                  size: layout.size,
                                  background: (transform) =>
                                      _GridPainter(transform, palette.hairline),
                                  child: _Board(
                                    layout: layout,
                                    pads: portalPads(
                                      layout,
                                      workspace.pipes,
                                      liaisons: pipeLiaisons(workspace),
                                    ),
                                    person: workspace.person,
                                    hidden: _hidden,
                                    focus: focus,
                                    onHover: _hovered,
                                    onTap: _open,
                                  ),
                                ),
                              ),
                              if (cardChip != null)
                                _cardFor(workspace, cardChip, phone: phone),
                            ],
                          ),
                        ),
                      ],
                    );
                  },
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// The board at its own size: boxes and lines painted, chips as widgets
/// to hover and click.
class _Board extends StatelessWidget {
  const _Board({
    required this.layout,
    this.pads = const [],
    required this.person,
    required this.hidden,
    required this.focus,
    required this.onHover,
    required this.onTap,
    this.phone = false,
  });

  final MapLayout layout;

  /// On a phone: pads a finger can tap, and a pipe opened closes the Map.
  final bool phone;

  /// The projects' pipes, each a portal pad on its project's box.
  final List<PortalPad> pads;

  /// The person, over the admins: they can message every agent, so no
  /// line leads to them.
  final Person person;
  final Set<MapLayer> hidden;
  final String? focus;
  final ValueChanged<String?> onHover;
  final ValueChanged<MapNode> onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final focus = this.focus;
    final lit = focus == null ? null : {focus, ...?layout.related[focus]};
    return Stack(
      children: [
        Positioned.fill(
          child: RepaintBoundary(
            child: CustomPaint(
              painter: _BoardPainter(
                layout: layout,
                pads: pads,
                palette: palette,
                hidden: hidden,
                focus: focus,
              ),
            ),
          ),
        ),
        for (final pad in pads)
          Positioned.fromRect(
            // A finger's height on a phone, the pad drawn in its middle.
            rect: phone
                ? Rect.fromCenter(
                    center: pad.rect.center,
                    width: pad.rect.width,
                    height: math.max(pad.rect.height, Touch.target),
                  )
                : pad.rect,
            child: AnimatedOpacity(
              duration: Motion.fast,
              opacity: lit == null ? 1 : 0.3,
              child: _PortalPadView(pad: pad, phone: phone),
            ),
          ),
        if (layout.you case final you?)
          Positioned.fromRect(
            rect: you,
            child: AnimatedOpacity(
              duration: Motion.fast,
              opacity: lit == null ? 1 : 0.3,
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  PersonAvatar(person: person, size: 32),
                  const SizedBox(height: Space.xs),
                  Text(
                    'You',
                    style: TextStyles.label.copyWith(color: palette.text),
                  ),
                ],
              ),
            ),
          ),
        for (final chip in layout.chips)
          Positioned.fromRect(
            rect: chip.rect,
            child: AnimatedOpacity(
              duration: Motion.fast,
              opacity: lit == null || lit.contains(chip.id) ? 1 : 0.3,
              child: MouseRegion(
                cursor: SystemMouseCursors.click,
                onEnter: (_) => onHover(chip.id),
                onExit: (_) {
                  if (focus == chip.id) onHover(null);
                },
                child: GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTap: () => onTap(chip.node),
                  child: _Chip(chip: chip, focused: focus == chip.id),
                ),
              ),
            ),
          ),
      ],
    );
  }
}

/// A node on the board: an agent (its color down the left, its name, role
/// and badges), a secret, or Claktory's records.
class _Chip extends StatelessWidget {
  const _Chip({required this.chip, required this.focused});

  final MapChip chip;
  final bool focused;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final node = chip.node;
    final agent = node.kind == MapNodeKind.MAP_NODE_KIND_AGENT;
    final border = BoxDecoration(
      color: palette.canvas,
      borderRadius: BorderRadius.circular(Radii.control),
      border: Border.all(color: focused ? palette.textMuted : palette.hairline),
    );
    if (!agent) {
      final secret = node.kind == MapNodeKind.MAP_NODE_KIND_SECRET;
      final app = node.kind == MapNodeKind.MAP_NODE_KIND_APP;
      final color = mapLayerColor(
        secret
            ? MapLayer.secrets
            : app
            ? MapLayer.apps
            : MapLayer.data,
        palette,
      );
      return Container(
        decoration: border,
        padding: const EdgeInsets.symmetric(horizontal: Space.s),
        child: Row(
          children: [
            ExtensionGlyph(
              icon: secret
                  ? 'key-round'
                  : app
                  ? node.icon
                  : 'database',
              name: app ? node.label : '',
              color: color,
              size: 14,
            ),
            const SizedBox(width: Space.s - 2),
            Expanded(
              child: Text(
                node.label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyles.caption.copyWith(
                  color: palette.text,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ),
          ],
        ),
      );
    }
    final color = agentColor(node.color);
    return Container(
      decoration: border,
      clipBehavior: Clip.antiAlias,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Container(width: 3, color: color),
          Expanded(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(
                Space.s,
                Space.xs + 1,
                Space.s,
                Space.xs,
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  Text(
                    node.label,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyles.label.copyWith(
                      color: palette.text,
                      height: 1.3,
                    ),
                  ),
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          node.detail,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyles.caption.copyWith(
                            color: palette.textMuted,
                            height: 1.3,
                          ),
                        ),
                      ),
                      for (final badge in node.badges) ...[
                        const SizedBox(width: Space.xs),
                        ExtensionGlyph(
                          icon: badge.icon,
                          name: badge.label,
                          color: _quiet.contains(badge.key)
                              ? palette.textFaint
                              : palette.textMuted,
                          size: 12,
                        ),
                      ],
                    ],
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// Badges most agents have (both are on by default): fainter than the rest.
const _quiet = {'web', 'tree'};

/// Boxes, labels, lines, dots and chevrons.
class _BoardPainter extends CustomPainter {
  _BoardPainter({
    required this.layout,
    this.pads = const [],
    required this.palette,
    required this.hidden,
    required this.focus,
  }) : hiddenKey = Set.of(hidden);

  final MapLayout layout;
  final List<PortalPad> pads;
  final Palette palette;
  final Set<MapLayer> hidden;
  final Set<MapLayer> hiddenKey;
  final String? focus;

  @override
  void paint(Canvas canvas, Size size) {
    final focus = this.focus;
    final reads = focus == null ? null : layout.reads[focus];
    final writes = focus == null ? null : layout.writes[focus];
    final data = mapLayerColor(MapLayer.data, palette);

    for (final box in layout.boxes) {
      final rect = box.rect;
      switch (box.kind) {
        case MapBoxKind.workspace || MapBoxKind.project:
          final shape = RRect.fromRectAndRadius(
            rect,
            const Radius.circular(Radii.panel),
          );
          canvas.drawRRect(shape, Paint()..color = palette.sidebar);
          canvas.drawRRect(shape, _stroke(palette.hairline, 1));
          // Short of its pipes' pads, at its top right.
          var padsFrom = rect.right;
          for (final pad in pads) {
            if (pad.boxId == box.id) {
              padsFrom = math.min(padsFrom, pad.rect.left - Space.s);
            }
          }
          _text(
            canvas,
            box.label,
            TextStyles.label.copyWith(
              color: palette.text,
              fontWeight: FontWeight.w600,
            ),
            Offset(rect.left + _titleInset(box), rect.top + mapCell + 1),
            math.min(padsFrom, rect.right - mapCell) -
                rect.left -
                _titleInset(box),
          );
          _text(
            canvas,
            box.detail,
            TextStyles.caption.copyWith(color: palette.textFaint),
            Offset(rect.left + _titleInset(box), rect.top + mapCell * 3 + 2),
            rect.width - _titleInset(box) - mapCell,
          );
        case MapBoxKind.folder ||
            MapBoxKind.vault ||
            MapBoxKind.admins ||
            MapBoxKind.apps:
          final writing = writes?.contains(box.id) ?? false;
          final reading = reads?.contains(box.id) ?? false;
          if (!box.outlined) {
            if (writing) {
              // A project's own folder with no other: the project's box.
              final whole = layout.boxes.firstWhere(
                (other) =>
                    other.id == box.id && other.kind == MapBoxKind.project,
                orElse: () => box,
              );
              canvas.drawRRect(
                RRect.fromRectAndRadius(
                  whole.rect.deflate(1),
                  const Radius.circular(Radii.panel - 1),
                ),
                _stroke(data, 1.5),
              );
            }
            continue;
          }
          final shape = RRect.fromRectAndRadius(
            rect,
            const Radius.circular(Radii.control),
          );
          canvas.drawRRect(
            shape,
            Paint()
              ..color =
                  box.kind == MapBoxKind.vault || box.kind == MapBoxKind.apps
                  ? palette.sidebar
                  : palette.canvas.withValues(alpha: 0.6),
          );
          canvas.drawRRect(
            shape,
            writing
                ? _stroke(data, 1.5)
                : reading
                ? _stroke(data.withValues(alpha: 0.55), 1)
                : _stroke(palette.hairline, 1),
          );
          _text(
            canvas,
            box.label,
            TextStyles.caption.copyWith(
              color: writing || reading ? data : palette.textMuted,
              fontWeight: FontWeight.w500,
            ),
            Offset(rect.left + mapCell, rect.top + mapCell - 2),
            rect.width - mapCell * 2,
          );
      }
    }

    // Each pipe leaves the board: a line from its pad up through the top of
    // its project's box, ending in a ring, as somewhere else begins.
    for (final pad in pads) {
      final color = pad.closed ? palette.textFaint : pipeColor;
      final paint = _stroke(
        focus == null ? color : color.withValues(alpha: 0.3),
        1.5,
      );
      final x = pad.rect.center.dx;
      final end = Offset(x, pad.rect.top - portalReach);
      canvas
        ..drawLine(Offset(x, pad.rect.top), end, paint)
        ..drawCircle(end, 2.5, paint);
    }

    // Lines: the dimmed first, so the lit ones run over them.
    bool shown(MapLayer layer) => !hidden.contains(layer);
    bool lit(Set<String> ends) => focus == null || ends.contains(focus);
    for (final pass in [false, true]) {
      for (final wire in layout.wires) {
        if (!shown(wire.layer) || lit(wire.ends) != pass) continue;
        final color = mapLayerColor(wire.layer, palette);
        final paint = _stroke(
          pass ? color : color.withValues(alpha: 0.22),
          wire.bus
              ? 2.5
              : focus != null && pass
              ? 1.75
              : 1.25,
        )..strokeCap = StrokeCap.square;
        final path = Path()..moveTo(wire.points.first.dx, wire.points.first.dy);
        for (final point in wire.points.skip(1)) {
          path.lineTo(point.dx, point.dy);
        }
        canvas.drawPath(path, paint);
        for (final arrow in wire.arrows) {
          _chevron(canvas, arrow, paint.color);
        }
      }
      for (final dot in layout.dots) {
        if (!shown(dot.layer) || lit(dot.ends) != pass) continue;
        final color = mapLayerColor(dot.layer, palette);
        canvas.drawCircle(
          dot.at,
          2.75,
          Paint()..color = pass ? color : color.withValues(alpha: 0.22),
        );
      }
    }
  }

  static double _titleInset(MapBox box) {
    // Titles line up with the chips, right of the bus.
    return box.kind == MapBoxKind.workspace || box.kind == MapBoxKind.project
        ? mapCell * 4
        : mapCell;
  }

  static Paint _stroke(Color color, double width) => Paint()
    ..color = color
    ..style = PaintingStyle.stroke
    ..strokeWidth = width
    ..strokeJoin = StrokeJoin.miter;

  void _chevron(Canvas canvas, MapArrow arrow, Color color) {
    final d = arrow.direction;
    final side = Offset(-d.dy, d.dx);
    final tip = arrow.at + d * 3;
    final path = Path()
      ..moveTo((tip - d * 5 + side * 4).dx, (tip - d * 5 + side * 4).dy)
      ..lineTo(tip.dx, tip.dy)
      ..lineTo((tip - d * 5 - side * 4).dx, (tip - d * 5 - side * 4).dy);
    canvas.drawPath(
      path,
      _stroke(color, 1.5)
        ..strokeCap = StrokeCap.round
        ..strokeJoin = StrokeJoin.round,
    );
  }

  static void _text(
    Canvas canvas,
    String text,
    TextStyle style,
    Offset at,
    double width,
  ) {
    if (text.isEmpty || width <= 0) return;
    final painter = TextPainter(
      text: TextSpan(text: text, style: style),
      textDirection: TextDirection.ltr,
      maxLines: 1,
      ellipsis: '…',
    )..layout(maxWidth: width);
    painter.paint(canvas, at);
    painter.dispose();
  }

  @override
  bool shouldRepaint(_BoardPainter old) =>
      old.layout != layout ||
      !_samePads(old.pads, pads) ||
      old.palette != palette ||
      old.focus != focus ||
      !old.hiddenKey.containsAll(hidden) ||
      old.hiddenKey.length != hidden.length;
}

bool _samePads(List<PortalPad> a, List<PortalPad> b) {
  if (a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i].rect != b[i].rect ||
        a[i].closed != b[i].closed ||
        a[i].pipe.name != b[i].pipe.name) {
      return false;
    }
  }
  return true;
}

/// The agents bound to a pipe the workspace has (Agent.pipe_id), by the
/// pipe's id: each shows on its pipe's pad rather than as a chip.
Map<String, Agent> pipeLiaisons(Workspace workspace) {
  final pipes = {for (final pipe in workspace.pipes) pipe.id};
  return {
    for (final agent in workspace.agents)
      if (pipes.contains(agent.pipeId)) agent.pipeId: agent,
  };
}

/// [map] without the nodes [ids] and their lines.
@visibleForTesting
ClaktoryMap mapWithout(ClaktoryMap map, Set<String> ids) {
  if (ids.isEmpty || !map.nodes.any((node) => ids.contains(node.id))) {
    return map;
  }
  return ClaktoryMap(
    nodes: [
      for (final node in map.nodes)
        if (!ids.contains(node.id)) node,
    ],
    edges: [
      for (final edge in map.edges)
        if (!ids.contains(edge.fromId) && !ids.contains(edge.toId)) edge,
    ],
  );
}

/// How far a pipe's line reaches out of its project's box, past its top.
const portalReach = mapCell * 2;

/// A pipe on the Map: a pad on its project's box, at [rect].
class PortalPad {
  const PortalPad({
    required this.pipe,
    required this.boxId,
    required this.rect,
    required this.label,
    this.liaison,
  });

  final Pipe pipe;

  /// Its liaison, whose face ends the pad.
  final Agent? liaison;

  /// Its project's box ("project:…").
  final String boxId;
  final Rect rect;

  /// The other Claktory's name; the pipe's own before it's known. Empty
  /// when there's room only for the portal.
  final String label;

  bool get closed => pipe.state == PipeState.PIPE_STATE_CLOSED;
}

/// Each project's pipes as pads along the top right of its box, on its
/// title's line, right to left in the order they were made. They take at
/// most half the box; past that, they show only their portals.
@visibleForTesting
List<PortalPad> portalPads(
  MapLayout layout,
  List<Pipe> pipes, {
  Map<String, Agent> liaisons = const {},
}) {
  if (pipes.isEmpty) return const [];
  final style = TextStyles.caption.copyWith(fontWeight: FontWeight.w500);
  final pads = <PortalPad>[];
  for (final box in layout.boxes) {
    if (box.kind != MapBoxKind.project) continue;
    final here = [
      for (final pipe in pipes)
        if ('project:${pipe.projectId}' == box.id) pipe,
    ]..sort((a, b) => a.createdMs.compareTo(b.createdMs));
    if (here.isEmpty) continue;
    final room = box.rect.width / 2;
    final labels = [
      for (final pipe in here)
        pipe.peer.claktoryName.isNotEmpty ? pipe.peer.claktoryName : pipe.name,
    ];
    final widths = [
      for (var i = 0; i < here.length; i++)
        math.min(
          _padMaxWidth + (liaisons.containsKey(here[i].id) ? _padFace : 0),
          _padBare +
              _textWidth(labels[i], style) +
              (liaisons.containsKey(here[i].id) ? _padFace : 0),
        ),
    ];
    final gaps = Space.xs * (here.length - 1);
    final full = widths.fold(0.0, (sum, width) => sum + width) + gaps <= room;
    var right = box.rect.right - mapCell;
    for (var i = 0; i < here.length; i++) {
      final width = full ? widths[i] : _padHeight;
      final left = right - width;
      if (left < box.rect.left + mapCell * 4) break;
      pads.add(
        PortalPad(
          pipe: here[i],
          boxId: box.id,
          rect: Rect.fromLTWH(
            left,
            box.rect.top + mapCell - 1,
            width,
            _padHeight,
          ),
          label: full ? labels[i] : '',
          liaison: full ? liaisons[here[i].id] : null,
        ),
      );
      right = left - Space.xs;
    }
  }
  return pads;
}

const _padHeight = 20.0;

/// What a liaison's face adds to its pipe's pad: the space before it and
/// the face.
const _padFace = Space.xs + 14.0;
const _padMaxWidth = 150.0;

/// A pad's width besides its label: padding, the portal and the space
/// after it, and the border.
const _padBare = Space.s + 12 + Space.xs + Space.s + 2;

double _textWidth(String text, TextStyle style) {
  final painter = TextPainter(
    text: TextSpan(text: text, style: style),
    textDirection: TextDirection.ltr,
    maxLines: 1,
  )..layout();
  final width = painter.width;
  painter.dispose();
  return width.ceilToDouble();
}

/// A pipe's pad on the Map: its portal and the other Claktory's name, in
/// the pipe's color. A click opens it; hovering shows its name and where
/// it stands.
class _PortalPadView extends StatelessWidget {
  const _PortalPadView({required this.pad, this.phone = false});

  final PortalPad pad;

  /// In a finger's taller box, drawn at its own height in the middle; a
  /// tap opens the pipe in place of the Map.
  final bool phone;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final pipe = pad.pipe;
    final color = pad.closed ? palette.textFaint : pipeColor;
    final status = pipeStatus(pipe);
    final liaison = pad.liaison;
    return HoverCard(
      card: (context) => Text(
        [
          pipe.name,
          if (status.isNotEmpty) status,
          if (liaison != null) liaison.name,
        ].join(' · '),
      ),
      child: Semantics(
        button: true,
        label: 'Pipe ${pipe.name}',
        child: Pressable(
          onTap: () {
            openPipe(context, pipe);
            // On a phone, the pipe's conversation in front.
            if (phone) PhoneNavigation.of(context)?.value = true;
          },
          builder: (context, hovered) => _padHeightOn(
            phone,
            AnimatedContainer(
              duration: Motion.fast,
              curve: Curves.easeOut,
              padding: const EdgeInsets.symmetric(horizontal: Space.s),
              decoration: BoxDecoration(
                color: hovered ? palette.hover : palette.canvas,
                borderRadius: BorderRadius.circular(_padHeight / 2),
                border: Border.all(color: color),
              ),
              child: Row(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  PortalGlyph(color: color, size: 12),
                  if (pad.label.isNotEmpty) ...[
                    const SizedBox(width: Space.xs),
                    Flexible(
                      child: Text(
                        pad.label,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyles.caption.copyWith(
                          color: palette.text,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                    ),
                  ],
                  if (liaison != null) ...[
                    const SizedBox(width: Space.xs),
                    AgentCharacter(agent: identityOf(liaison), size: 14),
                  ],
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// [pad] as it's drawn: in the middle of a phone's taller box, at its own
/// height; elsewhere filling its box.
Widget _padHeightOn(bool phone, Widget pad) => phone
    ? Center(
        child: SizedBox(height: _padHeight, child: pad),
      )
    : pad;

/// A faint dot every two cells, like a board's grid, at every zoom.
class _GridPainter extends CustomPainter {
  _GridPainter(this.transform, this.color);

  final Matrix4 transform;
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final scale = transform.getMaxScaleOnAxis();
    var step = mapCell * 2 * scale;
    // Never denser than every 12 pixels on screen.
    while (step < 12) {
      step *= 2;
    }
    final origin = Offset(transform.storage[12], transform.storage[13]);
    final x0 = origin.dx % step;
    final y0 = origin.dy % step;
    final paint = Paint()..color = color;
    final radius = math.max(0.6, math.min(1.0, scale));
    for (var y = y0; y < size.height; y += step) {
      for (var x = x0; x < size.width; x += step) {
        canvas.drawCircle(Offset(x, y), radius, paint);
      }
    }
  }

  @override
  bool shouldRepaint(_GridPainter old) =>
      old.transform != transform || old.color != color;
}

/// The key, along the top: a switch for each kind of line, with its color,
/// then the zoom; under them, what the badges on agents mean (only what
/// gives an agent reach or power; its card lists all its extensions).
class _Key extends StatelessWidget {
  const _Key({
    required this.layout,
    required this.hidden,
    required this.phone,
    required this.canvas,
    required this.onToggle,
  });

  final MapLayout layout;
  final Set<MapLayer> hidden;
  final bool phone;
  final CanvasController canvas;
  final ValueChanged<MapLayer> onToggle;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final layers = {for (final wire in layout.wires) wire.layer};
    final badges = <String, MapBadge>{};
    for (final chip in layout.chips) {
      for (final badge in chip.node.badges) {
        badges.putIfAbsent(badge.key, () => badge);
      }
    }
    final switches = [
      for (final layer in MapLayer.values)
        if (layers.contains(layer))
          Padding(
            padding: const EdgeInsets.only(right: Space.xs),
            child: _LayerSwitch(
              layer: layer,
              shown: !hidden.contains(layer),
              onToggle: () => onToggle(layer),
            ),
          ),
    ];
    final caption = TextStyles.caption.copyWith(color: palette.textFaint);
    return Unselectable(
      child: Container(
        padding: const EdgeInsets.fromLTRB(Space.m, Space.s, Space.m, Space.s),
        decoration: BoxDecoration(
          border: Border(bottom: BorderSide(color: palette.hairline)),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Expanded(
                  child: SingleChildScrollView(
                    scrollDirection: Axis.horizontal,
                    child: Row(children: switches),
                  ),
                ),
                if (!phone) _ZoomControls(controller: canvas),
              ],
            ),
            // On a phone, room for the board: the switches only.
            if (!phone) ...[
              const SizedBox(height: Space.xs),
              Wrap(
                spacing: Space.m,
                runSpacing: Space.xs,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  if (badges.isNotEmpty)
                    Text('Badges: extra reach or power', style: caption),
                  for (final badge in badges.values)
                    Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        ExtensionGlyph(
                          icon: badge.icon,
                          name: badge.label,
                          color: palette.textFaint,
                          size: 12,
                        ),
                        const SizedBox(width: Space.xs),
                        Text(badge.label, style: caption),
                      ],
                    ),
                  Text(
                    'Agents can read their whole project but change files '
                    'only in their own folder. Hover an agent to see '
                    'everything it has.',
                    style: caption,
                  ),
                ],
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// A kind of line, its color, and whether it shows: a quiet pill that's
/// tinted while on.
class _LayerSwitch extends StatelessWidget {
  const _LayerSwitch({
    required this.layer,
    required this.shown,
    required this.onToggle,
  });

  final MapLayer layer;
  final bool shown;
  final VoidCallback onToggle;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final color = mapLayerColor(layer, palette);
    return Semantics(
      toggled: shown,
      label: mapLayerNames[layer],
      excludeSemantics: true,
      onTap: onToggle,
      child: Pressable(
        onTap: onToggle,
        builder: (context, hovered) => AnimatedContainer(
          duration: Motion.fast,
          height: 28,
          padding: const EdgeInsets.symmetric(horizontal: Space.s),
          decoration: BoxDecoration(
            color: shown
                ? palette.selected
                : hovered
                ? palette.hover
                : null,
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              _Swatch(
                color: shown ? color : color.withValues(alpha: 0.3),
                bus: layer == MapLayer.messages,
              ),
              const SizedBox(width: Space.s - 2),
              Text(
                mapLayerNames[layer]!,
                style: TextStyles.caption.copyWith(
                  color: shown
                      ? palette.text
                      : hovered
                      ? palette.textMuted
                      : palette.textFaint,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A short stretch of a layer's line: a bus with a tap, for messages.
class _Swatch extends StatelessWidget {
  const _Swatch({required this.color, this.bus = false});

  final Color color;
  final bool bus;

  @override
  Widget build(BuildContext context) => CustomPaint(
    size: const Size(20, 12),
    painter: _SwatchPainter(color, bus),
  );
}

class _SwatchPainter extends CustomPainter {
  const _SwatchPainter(this.color, this.bus);

  final Color color;
  final bool bus;

  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.25;
    final y = size.height / 2;
    if (bus) {
      canvas.drawLine(
        Offset(0, y - 3),
        Offset(size.width, y - 3),
        paint..strokeWidth = 2.5,
      );
      canvas.drawLine(
        Offset(size.width / 2, y - 3),
        Offset(size.width / 2, size.height),
        paint..strokeWidth = 1.25,
      );
      canvas.drawCircle(
        Offset(size.width / 2, y - 3),
        2.75,
        Paint()..color = color,
      );
      return;
    }
    canvas.drawLine(Offset(0, y), Offset(size.width, y), paint);
  }

  @override
  bool shouldRepaint(_SwatchPainter old) =>
      old.color != color || old.bus != bus;
}

/// Zoom out, fit and zoom in, over the board's bottom right.
class _ZoomControls extends StatelessWidget {
  const _ZoomControls({required this.controller});

  final CanvasController controller;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Unselectable(
      child: SizedBox(
        child: ListenableBuilder(
          listenable: controller,
          builder: (context, _) => Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              GlyphButton(
                label: 'Zoom out',
                active: false,
                onTap: controller.zoomOut,
                glyph: (color) =>
                    ExtensionGlyph(icon: 'minus', name: '', color: color),
              ),
              Pressable(
                onTap: controller.fit,
                builder: (context, hovered) => Container(
                  height: 28,
                  constraints: const BoxConstraints(minWidth: 48),
                  alignment: Alignment.center,
                  padding: const EdgeInsets.symmetric(horizontal: Space.xs),
                  decoration: BoxDecoration(
                    color: hovered ? palette.hover : null,
                    borderRadius: BorderRadius.circular(Radii.control),
                  ),
                  child: Text(
                    controller.fitted
                        ? 'Fit'
                        : '${(controller.scale * 100).round()}%',
                    style: TextStyles.caption.copyWith(
                      color: hovered ? palette.text : palette.textMuted,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    ),
                  ),
                ),
              ),
              GlyphButton(
                label: 'Zoom in',
                active: false,
                onTap: controller.zoomIn,
                glyph: (color) =>
                    ExtensionGlyph(icon: 'plus', name: '', color: color),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Fit, over a phone's board at its foot: the whole map in view again.
/// Shows the zoom while it isn't fitted.
class _FitButton extends StatelessWidget {
  const _FitButton({required this.controller});

  final CanvasController controller;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ListenableBuilder(
      listenable: controller,
      builder: (context, _) => Semantics(
        button: true,
        label: 'Fit',
        value: controller.fitted
            ? null
            : '${(controller.scale * 100).round()}%',
        excludeSemantics: true,
        onTap: controller.fit,
        child: Pressable(
          onTap: controller.fit,
          builder: (context, pressed) => Container(
            constraints: const BoxConstraints(
              minHeight: Touch.target,
              minWidth: Touch.target + Space.l,
            ),
            alignment: Alignment.center,
            padding: const EdgeInsets.symmetric(horizontal: Space.l),
            decoration: BoxDecoration(
              color: pressed ? palette.selected : palette.sidebar,
              borderRadius: BorderRadius.circular(Touch.target / 2),
              border: Border.all(color: palette.hairline),
              boxShadow: floatingShadow,
            ),
            child: Text(
              controller.fitted
                  ? 'Fit'
                  : 'Fit · ${(controller.scale * 100).round()}%',
              style: PhoneType.label.copyWith(
                color: palette.text,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// A phone's Lines sheet: a switch for each kind of line on the map, then
/// what the badges on agents mean.
class _LinesPanel extends StatelessWidget {
  const _LinesPanel({
    required this.layers,
    required this.badges,
    required this.hidden,
    required this.onToggle,
  });

  final List<MapLayer> layers;
  final List<MapBadge> badges;
  final Set<MapLayer> hidden;
  final ValueChanged<MapLayer> onToggle;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final caption = PhoneType.secondary.copyWith(color: palette.textMuted);
    return SingleChildScrollView(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final layer in layers)
            ToggleTap(
              value: !hidden.contains(layer),
              onChanged: (_) => onToggle(layer),
              child: ConstrainedBox(
                constraints: const BoxConstraints(minHeight: Touch.row),
                child: Row(
                  children: [
                    _Swatch(
                      color: mapLayerColor(layer, palette),
                      bus: layer == MapLayer.messages,
                    ),
                    const SizedBox(width: Space.m),
                    Expanded(
                      child: Text(
                        mapLayerNames[layer]!,
                        style: PhoneType.body.copyWith(color: palette.text),
                      ),
                    ),
                    Toggle(
                      value: !hidden.contains(layer),
                      label: mapLayerNames[layer]!,
                      onChanged: (_) => onToggle(layer),
                    ),
                  ],
                ),
              ),
            ),
          if (badges.isNotEmpty) ...[
            const SizedBox(height: Space.l),
            Text(
              'Badges: extra reach or power',
              style: PhoneType.caption.copyWith(
                color: palette.textMuted,
                fontWeight: FontWeight.w500,
              ),
            ),
            const SizedBox(height: Space.s),
            for (final badge in badges)
              Padding(
                padding: const EdgeInsets.only(bottom: Space.s),
                child: Row(
                  children: [
                    SizedBox(
                      width: 20,
                      child: ExtensionGlyph(
                        icon: badge.icon,
                        name: badge.label,
                        color: palette.textMuted,
                        size: 16,
                      ),
                    ),
                    const SizedBox(width: Space.m),
                    Expanded(child: Text(badge.label, style: caption)),
                  ],
                ),
              ),
          ],
          const SizedBox(height: Space.m),
          Text(
            'Agents can read their whole project but change files only in '
            'their own folder. Tap an agent to see everything it has.',
            style: caption,
          ),
        ],
      ),
    );
  }
}

/// Everything on the map as a list, on a phone: a search field, then each
/// project's agents (their name, role, lead, whom they message and their
/// secrets), then the secrets, apps and records. A tap shows one on the
/// map, lit up, an agent with its card.
class _MapList extends StatefulWidget {
  const _MapList({required this.onPick});

  final ValueChanged<String> onPick;

  @override
  State<_MapList> createState() => _MapListState();
}

class _MapListState extends State<_MapList> {
  final _query = TextEditingController();

  @override
  void dispose() {
    _query.dispose();
    super.dispose();
  }

  bool _matches(List<String> words) {
    final query = _query.text.trim().toLowerCase();
    if (query.isEmpty) return true;
    final haystack = words.join(' ').toLowerCase();
    return query
        .split(RegExp(r'\s+'))
        .where((word) => word.isNotEmpty)
        .every(haystack.contains);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: Listenable.merge([backend.workspace, _query]),
      builder: (context, _) {
        final workspace = backend.workspace.workspace;
        final nodes = workspace.map.nodes;
        final places = [
          for (final node in nodes)
            if (node.kind == MapNodeKind.MAP_NODE_KIND_WORKSPACE ||
                node.kind == MapNodeKind.MAP_NODE_KIND_PROJECT)
              node,
        ];
        String placeOf(MapNode agent) {
          for (final place in places) {
            final here = place.kind == MapNodeKind.MAP_NODE_KIND_WORKSPACE
                ? agent.projectId.isEmpty
                : place.refId == agent.projectId;
            if (here) return place.label;
          }
          return '';
        }

        final agents = [
          for (final node in nodes)
            if (node.kind == MapNodeKind.MAP_NODE_KIND_AGENT &&
                _matches([node.label, node.detail, placeOf(node)]))
              node,
        ];
        final others = [
          for (final node in nodes)
            if ((node.kind == MapNodeKind.MAP_NODE_KIND_SECRET ||
                    node.kind == MapNodeKind.MAP_NODE_KIND_APP ||
                    node.kind == MapNodeKind.MAP_NODE_KIND_DATA) &&
                _matches([node.label, node.detail]))
              node,
        ];
        // In the sidebar's order: the workspace's agents, then each
        // project's.
        final order = sidebarOrder(workspace);
        int rank(String id) => order[id] ?? order.length;
        agents.sort((a, b) => rank(a.id).compareTo(rank(b.id)));
        final groups = <String, List<MapNode>>{};
        for (final agent in agents) {
          (groups[agent.projectId] ??= []).add(agent);
        }
        final projects = groups.keys.toList()
          ..sort(
            (a, b) => a.isEmpty
                ? -1
                : b.isEmpty
                ? 1
                : rank('project:$a').compareTo(rank('project:$b')),
          );
        final heading = PhoneType.caption.copyWith(
          color: palette.textMuted,
          fontWeight: FontWeight.w500,
        );
        Widget title(String text) => Padding(
          padding: const EdgeInsets.fromLTRB(
            Touch.gutter,
            Space.l,
            Touch.gutter,
            Space.xs,
          ),
          child: Semantics(header: true, child: Text(text, style: heading)),
        );
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            PhoneHeader(
              leading: HeaderButton.back(
                onTap: () => PhoneRoutes.of(context)?.pop<void>(),
              ),
              title: Text(
                'On the map',
                style: PhoneType.emphasis.copyWith(color: palette.text),
              ),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(
                Touch.gutter,
                Space.s,
                Touch.gutter,
                0,
              ),
              child: TextField(
                controller: _query,
                placeholder: 'Search',
                kind: InputKind.text,
                autocorrect: false,
                textInputAction: TextInputAction.search,
              ),
            ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.only(bottom: Space.xl),
                children: [
                  if (agents.isEmpty && others.isEmpty)
                    Padding(
                      padding: const EdgeInsets.all(Touch.gutter),
                      child: Text(
                        'Nothing matches',
                        style: PhoneType.body.copyWith(
                          color: palette.textMuted,
                        ),
                      ),
                    ),
                  for (final project in projects) ...[
                    title(placeOf(groups[project]!.first)),
                    for (final agent in groups[project]!)
                      _ListRow(
                        node: agent,
                        relations: _relations(
                          mapAgentDetails(workspace, agent),
                        ),
                        onTap: () => widget.onPick(agent.id),
                      ),
                  ],
                  if (others.isNotEmpty) ...[
                    title('Secrets and records'),
                    for (final node in others)
                      _ListRow(
                        node: node,
                        relations: '',
                        onTap: () => widget.onPick(node.id),
                      ),
                  ],
                ],
              ),
            ),
          ],
        );
      },
    );
  }

  /// An agent's ties in a line: "Reports to Mike · Messages Sam, Iris ·
  /// 2 secrets".
  static String _relations(MapAgentDetails details) {
    String names(List<MapNode> nodes) => nodes.length <= 3
        ? nodes.map((node) => node.label).join(', ')
        : '${nodes.length} agents';
    return [
      if (details.lead case final lead?) 'Reports to ${lead.label}',
      if (details.reports.isNotEmpty) 'Leads ${names(details.reports)}',
      if (details.everyone)
        'Messages every agent'
      else if (details.messages.isNotEmpty)
        'Messages ${names(details.messages)}',
      if (details.secrets.length == 1)
        '1 secret'
      else if (details.secrets.length > 1)
        '${details.secrets.length} secrets',
    ].join(' · ');
  }
}

/// A row of the map's list: an agent (its color, name, role and ties), or
/// a secret, an app or Claktory's records with its glyph.
class _ListRow extends StatelessWidget {
  const _ListRow({
    required this.node,
    required this.relations,
    required this.onTap,
  });

  final MapNode node;
  final String relations;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final agent = node.kind == MapNodeKind.MAP_NODE_KIND_AGENT;
    final secret = node.kind == MapNodeKind.MAP_NODE_KIND_SECRET;
    final app = node.kind == MapNodeKind.MAP_NODE_KIND_APP;
    final mark = agent
        ? Container(
            width: 4,
            height: 32,
            decoration: BoxDecoration(
              color: agentColor(node.color),
              borderRadius: BorderRadius.circular(2),
            ),
          )
        : ExtensionGlyph(
            icon: secret
                ? 'key-round'
                : app
                ? node.icon
                : 'database',
            name: app ? node.label : '',
            color: mapLayerColor(
              secret
                  ? MapLayer.secrets
                  : app
                  ? MapLayer.apps
                  : MapLayer.data,
              palette,
            ),
            size: 16,
          );
    return Semantics(
      container: true,
      button: true,
      onTap: onTap,
      child: Pressable(
        onTap: onTap,
        builder: (context, pressed) => Container(
          constraints: const BoxConstraints(minHeight: Touch.row),
          color: pressed ? palette.hover : const Color(0x00000000),
          padding: const EdgeInsets.symmetric(
            horizontal: Touch.gutter,
            vertical: Space.s,
          ),
          child: Row(
            children: [
              SizedBox(width: 24, child: Center(child: mark)),
              const SizedBox(width: Space.m),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      node.label,
                      style: PhoneType.body.copyWith(color: palette.text),
                    ),
                    if (node.detail.isNotEmpty)
                      Text(
                        node.detail,
                        style: PhoneType.secondary.copyWith(
                          color: palette.textMuted,
                        ),
                      ),
                    if (relations.isNotEmpty)
                      Text(
                        relations,
                        style: PhoneType.caption.copyWith(
                          color: palette.textFaint,
                        ),
                      ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
