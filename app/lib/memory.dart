import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'extensions.dart';
import 'identity.dart';
import 'markdown.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'state.dart';

/// An agent's memory, read-only: its preferences (each rule with why and
/// when it was learned), its notes (a slug and summary that open to the
/// note), and what changed lately.
class MemoryView extends StatelessWidget {
  const MemoryView({super.key, required this.memory, this.now});

  final Memory memory;

  /// What "today" is; the clock's time unless given.
  final DateTime? now;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final faint = TextStyles.body.copyWith(color: palette.textFaint);
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
    final now = this.now ?? DateTime.now();
    final store = AppScope.of(context).workspace;
    Widget empty(String text) => Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.s,
        vertical: Space.xs,
      ),
      child: Text(text, style: faint),
    );
    return ListView(
      padding: const EdgeInsets.fromLTRB(Space.s, Space.s, Space.s, Space.l),
      children: [
        if (!readOnly) ...[
          const _Heading('Preferences'),
          if (memory.preferences.isEmpty) empty('No preferences yet.'),
          for (final preference in memory.preferences)
            _Preference(preference: preference, now: now),
        ],
        const _Heading('Notes'),
        if (memory.notes.isEmpty) empty('No notes yet.'),
        for (final note in memory.notes)
          _Note(key: ValueKey(note.slug), note: note, now: now),
        if (!readOnly && memory.changes.isNotEmpty) ...[
          const _Heading('Recent changes'),
          for (final change in memory.changes)
            _Change(change: change, store: store, now: now),
        ],
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
        style: TextStyles.caption.copyWith(
          color: palette.textFaint,
          fontWeight: FontWeight.w500,
        ),
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
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.s,
        vertical: Space.s - 2,
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: Space.xl,
            child: Text(
              '#${preference.id}',
              style: TextStyles.body.copyWith(
                color: palette.textFaint,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
          ),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  preference.rule,
                  style: TextStyles.body.copyWith(color: palette.text),
                ),
                if (preference.why.isNotEmpty)
                  Text(
                    'Why: ${preference.why}',
                    style: TextStyles.caption.copyWith(
                      color: palette.textMuted,
                    ),
                  ),
                Text(
                  [
                    'Learned ${_when(learned, now)}',
                    if (changed)
                      'changed ${_when(_date(preference.updated.toInt()), now)}',
                  ].join(' · '),
                  style: TextStyles.caption.copyWith(color: palette.textFaint),
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
  const _Note({super.key, required this.note, required this.now});

  final MemoryNote note;
  final DateTime now;

  @override
  State<_Note> createState() => _NoteState();
}

class _NoteState extends State<_Note> {
  var _open = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final note = widget.note;
    final edited = note.updated.toInt() - note.created.toInt() > 60000;
    return Pressable(
      onTap: () => setState(() => _open = !_open),
      builder: (context, hovered) => Container(
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
                      Text(
                        note.slug,
                        style: TextStyles.code.copyWith(
                          color: palette.textMuted,
                        ),
                      ),
                      if (note.summary.isNotEmpty)
                        Text(
                          note.summary,
                          style: TextStyles.body.copyWith(color: palette.text),
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
                          style: TextStyles.caption.copyWith(
                            color: palette.textFaint,
                          ),
                        ),
                      ),
                  ],
                ),
              ),
          ],
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
    final who = change.actor.isEmpty ? 'You' : actor?.name ?? change.actor;
    final time = _date(change.time.toInt());
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.s,
        vertical: Space.xs / 2,
      ),
      child: Text.rich(
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
        style: TextStyles.caption,
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

/// "today", "yesterday", or a day heading's date ("Mon 16 Nov").
String _when(DateTime time, DateTime now) {
  final label = dayLabel(time, now);
  return label == 'Today' || label == 'Yesterday' ? label.toLowerCase() : label;
}
