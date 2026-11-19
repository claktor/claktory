import 'package:flutter/widgets.dart';

import '../identity.dart';
import '../protocol/factory/v1/factory.pb.dart';
import 'clock.dart';
import 'identity.dart';
import 'painter.dart';
import 'pose.dart';

export 'identity.dart' show Mood, CharacterShape, CharacterSpec;

/// What [agent]'s face shows: the mood Factory resolved for it. From a
/// backend that doesn't say (or says something this app doesn't know),
/// only what the app knows for sure: working is thinking, else no model,
/// else news, else idle. Never failed, a tool or asleep.
Mood moodOf(Agent agent) {
  switch (agent.characterMood) {
    case CharacterMood.CHARACTER_MOOD_IDLE:
      return Mood.idle;
    case CharacterMood.CHARACTER_MOOD_THINKING:
      return Mood.thinking;
    case CharacterMood.CHARACTER_MOOD_WORKING:
      return Mood.working;
    case CharacterMood.CHARACTER_MOOD_NEWS:
      return Mood.news;
    case CharacterMood.CHARACTER_MOOD_FAILED:
      return Mood.failed;
    case CharacterMood.CHARACTER_MOOD_NO_MODEL:
      return Mood.noModel;
    case CharacterMood.CHARACTER_MOOD_ASLEEP:
      return Mood.asleep;
    default:
      break;
  }
  if (agent.status == AgentStatus.AGENT_STATUS_WORKING) {
    return Mood.thinking;
  }
  if (!agent.modelAvailable) return Mood.noModel;
  if (agent.unseen) return Mood.news;
  return Mood.idle;
}

/// A shape by its id; null for one this app doesn't know.
CharacterShape? shapeNamed(String id) {
  for (final shape in CharacterShape.values) {
    if (shape.name == id) return shape;
  }
  return null;
}

/// Who [agent] is, as a character: the shape and seed Factory resolved for
/// it. A shape this app doesn't know (or none) is a circle, for every agent
/// alike (no second defaults scheme here); the seed then only keeps blinks
/// and glances apart.
CharacterSpec specOf(Agent agent) => CharacterSpec(
  shape: shapeNamed(agent.avatarShape) ?? CharacterShape.circle,
  color: agentColor(agent.color),
  seed: agent.characterSeed != 0 ? agent.characterSeed : fnv1a(agent.id),
);

/// Who an agent is and nothing else: what a reference to it needs, so a
/// change in its mood or work doesn't rebuild what refers to it.
Agent identityOf(Agent agent) => Agent(
  id: agent.id,
  name: agent.name,
  role: agent.role,
  color: agent.color,
  avatarShape: agent.avatarShape,
  characterSeed: agent.characterSeed,
);

/// A mood change older than this when the app first sees it (after a
/// reconnect, say) happened while it wasn't looking: it's shown, not
/// played.
const freshChange = Duration(seconds: 2);

/// How a mood reads for assistive technology: "Researcher — thinking".
String moodLabel(Mood mood) => switch (mood) {
  Mood.idle => 'idle',
  Mood.thinking => 'thinking',
  Mood.working => 'tool running',
  Mood.news => 'has news',
  Mood.failed => 'last run failed',
  Mood.noModel => 'no model',
  Mood.asleep => 'asleep (idle)',
};

/// An agent's living character, in place of the dot that marked it. A
/// [live] one shows the agent's mood now (the sidebar, header and
/// Messaging); a reference (bubbles, owners, pickers) only rests and
/// blinks. Every copy shares the window's one clock and repaints alone,
/// without rebuilding anything around it.
class AgentCharacter extends StatefulWidget {
  const AgentCharacter({
    super.key,
    required this.agent,
    required this.size,
    this.live = false,
    this.mood,
  });

  final Agent agent;
  final double size;
  final bool live;

  /// Shown instead of the agent's own mood: the settings preview's samples.
  final Mood? mood;

  @override
  State<AgentCharacter> createState() => _AgentCharacterState();
}

class _AgentCharacterState extends State<AgentCharacter> {
  CharacterClock? _clock;
  MoodTrack? _track;
  CharacterSubscription? _subscription;
  late CharacterSpec _spec;

  /// While the shape changes: what it was, and when it began.
  CharacterSpec? _morphFrom;
  double _morphStart = 0;

  Mood get _mood => widget.mood ?? moodOf(widget.agent);

  /// The track key: a preview's sample moods mustn't move the agent's own.
  String get _key =>
      widget.mood == null ? widget.agent.id : '${widget.agent.id}#preview';

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _attach();
  }

  @override
  void didUpdateWidget(AgentCharacter old) {
    super.didUpdateWidget(old);
    if (old.agent.id != widget.agent.id ||
        old.live != widget.live ||
        (old.mood == null) != (widget.mood == null)) {
      _detach();
      _attach();
      return;
    }
    final next = specOf(widget.agent);
    final clock = _clock;
    if (clock != null && !clock.still && next.shape != _spec.shape) {
      // A new shape: a blink, and the body eases into the new one.
      _morphFrom = _spec;
      _morphStart = clock.now;
      _track?.reshape(clock.now);
      clock.wake();
    }
  }

  /// The shape being left and how far it's gone, while it changes.
  (CharacterShape, double)? _morph() {
    final from = _morphFrom;
    final clock = _clock;
    if (from == null || clock == null) return null;
    final progress = (clock.now - _morphStart) / shapeMorph;
    if (clock.still || progress >= 1 || progress < 0) {
      _morphFrom = null;
      return null;
    }
    return (from.shape, progress);
  }

  void _attach() {
    _spec = specOf(widget.agent);
    final clock = CharacterHost.of(context);
    if (clock == _clock && _track != null) return;
    _detach();
    _clock = clock;
    if (clock == null) {
      _track = MoodTrack(seed: _spec.seed, mood: _mood, compact: !widget.live);
      return;
    }
    final track = clock.track(_key, _spec.seed, _mood, live: widget.live);
    _track = track;
    _subscription = clock.subscribe(
      track: track,
      live: widget.live,
      size: widget.size,
    );
  }

  void _detach() {
    _subscription?.cancel();
    _subscription = null;
    _track = null;
  }

  @override
  void dispose() {
    _detach();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    _spec = specOf(widget.agent);
    final clock = _clock;
    final track = _track!;
    if (widget.live) {
      final since = widget.agent.characterMoodSinceMs.toInt();
      final age = DateTime.now().millisecondsSinceEpoch - since;
      final before = track.mood;
      track.change(
        _mood,
        clock?.now ?? 0,
        // Played only when it just happened; an old backend gives no time.
        fresh: since == 0 || age < freshChange.inMilliseconds,
      );
      // A sleeping clock wakes for the blend.
      if (track.mood != before) clock?.wake();
    }
    final subscription = _subscription;
    final name = widget.agent.name;
    return Semantics(
      label: widget.live ? '$name — ${moodLabel(_mood)}' : name,
      excludeSemantics: true,
      child: RepaintBoundary(
        child: CustomPaint(
          size: Size.square(widget.size),
          painter: CharacterPainter(
            spec: _spec,
            repaint: subscription,
            onPaint: subscription?.painted,
            morph: _morph,
            pose: clock == null
                ? () => track.pose(0, still: true)
                : () => track.pose(clock.now, still: clock.still),
          ),
        ),
      ),
    );
  }
}

/// The plain stand-in for an agent the app doesn't know (deleted, or from
/// elsewhere): a faint circle with still eyes, no identity invented.
class UnknownCharacter extends StatelessWidget {
  const UnknownCharacter({super.key, required this.size});

  final double size;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return CustomPaint(
      size: Size.square(size),
      painter: CharacterPainter(
        spec: CharacterSpec(
          shape: CharacterShape.circle,
          color: palette.textFaint,
          seed: 0,
        ),
        eyeColor: palette.canvas,
        pose: () => Pose.rest,
      ),
    );
  }
}

/// A character at rest, as a sample of a shape (the picker in agent
/// settings): no clock, no mood.
class StillCharacter extends StatelessWidget {
  const StillCharacter({super.key, required this.spec, required this.size});

  final CharacterSpec spec;
  final double size;

  @override
  Widget build(BuildContext context) => CustomPaint(
    size: Size.square(size),
    painter: CharacterPainter(spec: spec, pose: _rest),
  );

  static Pose _rest() => Pose.rest;
}
