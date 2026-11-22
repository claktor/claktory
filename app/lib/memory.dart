import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'extensions.dart';
import 'find.dart';
import 'identity.dart';
import 'mobile/metrics.dart';
import 'markdown.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'state.dart';

/// An agent's memory, read-only: its preferences (each rule with why and
/// when it was learned), its notes (a slug and summary that open to the
/// note), and what changed lately.
class MemoryView extends StatefulWidget {
  const MemoryView({super.key, required this.memory, this.now});

  final Memory memory;

  /// What "today" is; the clock's time unless given.
  final DateTime? now;

  @override
  State<MemoryView> createState() => _MemoryViewState();
}

class _MemoryViewState extends State<MemoryView> {
  final _scroll = ScrollController();

  /// Find in the memory (⌘F): its preferences (and why), its notes (their
  /// names, summaries and words) and what changed, as listed.
  late final _find = LocalFind(
    placeholder: 'Search this memory',
    items: () => _items,
    go: (key, _) {
      final place = _places[key];
      if (place == null) return;
      final index = _items.indexWhere((item) => item.$1 == key);
      bringIntoSight(
        _scroll,
        place,
        _items.length < 2 ? 0 : index / (_items.length - 1),
      );
    },
  );

  /// What can be found, as the last build listed it, and each one's place.
  var _items = const <(Object, String)>[];
  final _places = <Object, GlobalKey>{};

  @override
  void dispose() {
    _find.dispose();
    _scroll.dispose();
    super.dispose();
  }

  /// [child], the listed thing [key] with [text]: found in, and marked.
  Widget _findable(Object key, String text, Widget child) {
    (_building ??= []).add((key, text));
    return LocalFindMarks(
      key: _places.putIfAbsent(key, GlobalKey.new),
      find: _find,
      itemKey: key,
      child: child,
    );
  }

  List<(Object, String)>? _building;

  @override
  Widget build(BuildContext context) {
    final memory = widget.memory;
    _building = [];
    final list = _list(context, memory);
    _items = _building!;
    _building = null;
    final keys = {for (final item in _items) item.$1};
    _places.removeWhere((key, _) => !keys.contains(key));
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _find.refresh();
    });
    return FindScope(target: _find, child: list);
  }

  Widget _list(BuildContext context, Memory memory) {
    final palette = Palette.of(context);
    final faint = TypeScale.of(context).body.copyWith(color: palette.textFaint);
    // A read-only memory (such as the Factory manual) is only its notes:
    // nothing it learned, and nothing that changes.
    final readOnly = memory.readOnly;
    if (memory.notes.isEmpty &&
        (readOnly || memory.preferences.isEmpty && memory.changes.isEmpty)) {
      return Padding(
        padding: const EdgeInsets.all(Space.l),
        child: Text(
          readOnly ? 'Nothing in it.' : 'Nothing remembered yet.',
          style: faint,
        ),
      );
    }
    final now = widget.now ?? DateTime.now();
    final store = AppScope.of(context).workspace;
    Widget empty(String text) => Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.s,
        vertical: Space.xs,
      ),
      child: Text(text, style: faint),
    );
    return ListView(
      controller: _scroll,
      padding: const EdgeInsets.fromLTRB(Space.s, Space.s, Space.s, Space.l),
      children: [
        // Each selectable where the pointer is at any zoom.
        for (final child in <Widget>[
          if (!readOnly) ...[
            const _Heading('Preferences'),
            if (memory.preferences.isEmpty) empty('No preferences yet.'),
            for (final preference in memory.preferences)
              _findable(
                'preference:${preference.id}',
                [
                  preference.rule,
                  if (preference.why.isNotEmpty) 'Why: ${preference.why}',
                ].join('\n'),
                _Preference(preference: preference, now: now),
              ),
          ],
          const _Heading('Notes'),
          if (memory.notes.isEmpty) empty('No notes yet.'),
          for (final note in memory.notes)
            _findable(
              'note:${note.slug}',
              [note.slug, note.summary, note.body].join('\n'),
              _Note(
                key: ValueKey(note.slug),
                note: note,
                now: now,
                find: _find,
                findKey: 'note:${note.slug}',
              ),
            ),
          if (!readOnly && memory.changes.isNotEmpty) ...[
            const _Heading('Recent changes'),
            for (final (index, change) in memory.changes.indexed)
              _findable(
                'change:$index',
                '${_who(change, store)} ${changeSummary(change)}',
                _Change(change: change, store: store, now: now),
              ),
          ],
        ])
          ScrolledSelection(
            key: child.key == null ? null : ValueKey(child.key),
            child: child,
          ),
      ],
    );
  }
}

/// A section's name, like the checklist's day headings.
class _Heading extends StatelessWidget {
  const _Heading(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(Space.s, Space.m, Space.s, Space.xs),
      child: Text(
        text,
        style: TypeScale.of(context).caption
            .copyWith(color: palette.textFaint, fontWeight: FontWeight.w500),
      ),
    );
  }
}

class _Preference extends StatelessWidget {
  const _Preference({required this.preference, required this.now});

  final MemoryPreference preference;
  final DateTime now;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final learned = _date(preference.learned.toInt());
    final changed =
        preference.updated.toInt() - preference.learned.toInt() > 60000;
    // On a phone its number leads the line under it: a column of its own
    // would leave the rule little room at larger text.
    final phone = isPhoneLayout(context);
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.s,
        vertical: Space.s - 2,
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (!phone)
            SizedBox(
              width: Space.xl,
              child: Text(
                '#${preference.id}',
                style: TypeScale.of(context).body.copyWith(
                  color: palette.textFaint,
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
            ),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text.rich(
                  FindHighlight.markText(
                    context,
                    preference.rule,
                    TypeScale.of(context).body.copyWith(color: palette.text),
                  ),
                ),
                if (preference.why.isNotEmpty)
                  Text.rich(
                    FindHighlight.markText(
                      context,
                      'Why: ${preference.why}',
                      TypeScale.of(context).caption
                          .copyWith(color: palette.textMuted),
                    ),
                  ),
                Text(
                  [
                    if (phone) '#${preference.id}',
                    'Learned ${_when(learned, now)}',
                    if (changed)
                      'changed ${_when(_date(preference.updated.toInt()), now)}',
                  ].join(' · '),
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.textFaint),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// A note: its slug and summary, opening to its body.
class _Note extends StatefulWidget {
  const _Note({
    super.key,
    required this.note,
    required this.now,
    this.find,
    this.findKey = '',
  });

  final MemoryNote note;
  final DateTime now;

  /// The memory's find: a note found opens, to show its words.
  final LocalFind? find;
  final Object findKey;

  @override
  State<_Note> createState() => _NoteState();
}

class _NoteState extends State<_Note> {
  var _open = false;

  @override
  void initState() {
    super.initState();
    widget.find?.marks.addListener(_found);
  }

  @override
  void dispose() {
    widget.find?.marks.removeListener(_found);
    super.dispose();
  }

  /// Gone to by find: open, so the match in its words shows.
  void _found() {
    if (widget.find?.marks.value.at == widget.findKey && !_open && mounted) {
      setState(() => _open = true);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final note = widget.note;
    final edited = note.updated.toInt() - note.created.toInt() > 60000;
    return Semantics(
      container: true,
      expanded: _open,
      child: Pressable(
        onTap: () => setState(() => _open = !_open),
        builder: (context, hovered) => Container(
          constraints: BoxConstraints(
            minHeight: isPhoneLayout(context) ? Touch.target : 0,
          ),
          padding: const EdgeInsets.symmetric(
            horizontal: Space.s,
            vertical: Space.s - 2,
          ),
          decoration: BoxDecoration(
            color: hovered ? palette.hover : null,
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Padding(
                    padding: const EdgeInsets.only(top: 5),
                    child: Chevron(color: palette.textFaint, down: _open),
                  ),
                  const SizedBox(width: Space.s),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text.rich(
                          FindHighlight.markText(
                            context,
                            note.slug,
                            TypeScale.of(context).code
                                .copyWith(color: palette.textMuted),
                          ),
                        ),
                        if (note.summary.isNotEmpty)
                          Text.rich(
                            FindHighlight.markText(
                              context,
                              note.summary,
                              TypeScale.of(context).body
                                  .copyWith(color: palette.text),
                            ),
                          ),
                      ],
                    ),
                  ),
                ],
              ),
              if (_open)
                Padding(
                  padding: const EdgeInsets.fromLTRB(
                    Space.s + 10,
                    Space.s,
                    0,
                    Space.xs,
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      if (note.body.isNotEmpty) Markdown(note.body),
                      // Notes that were never written by anyone (a read-only
                      // memory's) have no times.
                      if (note.created > 0)
                        Padding(
                          padding: const EdgeInsets.only(top: Space.xs),
                          child: Text(
                            [
                              'Written ${_when(_date(note.created.toInt()), widget.now)}',
                              if (edited)
                                'edited ${_when(_date(note.updated.toInt()), widget.now)}',
                            ].join(' · '),
                            style: TypeScale.of(context).caption
                                .copyWith(color: palette.textFaint),
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

/// One change: when, who, what happened, to which preference or note.
class _Change extends StatelessWidget {
  const _Change({required this.change, required this.store, required this.now});

  final MemoryChange change;
  final WorkspaceStore store;
  final DateTime now;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final actor = change.actor.isEmpty ? null : store.agent(change.actor);
    final who = _who(change, store);
    final time = _date(change.time.toInt());
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.s,
        vertical: Space.xs / 2,
      ),
      child: Text.rich(
        FindHighlight.mark(
          context,
          TextSpan(
            children: [
              TextSpan(
                text: '${logTime(time, now: now)}  ',
                style: TextStyle(
                  color: palette.textFaint,
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
              TextSpan(
                text: '$who ',
                style: TextStyle(
                  color: actor == null
                      ? palette.textMuted
                      : agentColor(actor.color),
                  fontWeight: FontWeight.w500,
                ),
              ),
              TextSpan(
                text: changeSummary(change),
                style: TextStyle(color: palette.textMuted),
              ),
            ],
          ),
        ),
        style: TypeScale.of(context).caption,
      ),
    );
  }
}

/// What a change did, after who did it: "added preference #3: Keep replies
/// short", "wrote note deploy-steps: How to deploy".
String changeSummary(MemoryChange change) {
  final preference = '#${change.preferenceId}';
  final note = change.noteSlug;
  final (verb, subject) = switch (change.kind) {
    MemoryChangeKind.MEMORY_CHANGE_KIND_PREFERENCE_ADDED => (
      'added preference',
      preference,
    ),
    MemoryChangeKind.MEMORY_CHANGE_KIND_PREFERENCE_CHANGED => (
      'changed preference',
      preference,
    ),
    MemoryChangeKind.MEMORY_CHANGE_KIND_PREFERENCE_REMOVED => (
      'removed preference',
      preference,
    ),
    MemoryChangeKind.MEMORY_CHANGE_KIND_NOTE_WRITTEN => ('wrote note', note),
    MemoryChangeKind.MEMORY_CHANGE_KIND_NOTE_EDITED => ('edited note', note),
    MemoryChangeKind.MEMORY_CHANGE_KIND_NOTE_REMOVED => ('removed note', note),
    _ => ('changed its memory', ''),
  };
  return [
    verb,
    if (subject.isNotEmpty) ' $subject',
    if (change.text.isNotEmpty) ': ${change.text}',
  ].join();
}

DateTime _date(int ms) => DateTime.fromMillisecondsSinceEpoch(ms);

/// Who made [change]: the person, or the agent by name.
String _who(MemoryChange change, WorkspaceStore store) => change.actor.isEmpty
    ? 'You'
    : store.agent(change.actor)?.name ?? change.actor;

/// "today", "yesterday", or a day heading's date ("Mon 16 Nov").
String _when(DateTime time, DateTime now) {
  final label = dayLabel(time, now);
  return label == 'Today' || label == 'Yesterday' ? label.toLowerCase() : label;
}
