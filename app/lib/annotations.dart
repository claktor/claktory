import 'dart:async';
import 'dart:convert';
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'avatar.dart';
import 'backend.dart';
import 'card_answer.dart' show insteadLine;
import 'controls.dart';
import 'decision.dart';
import 'identity.dart';
import 'mobile/comment_editor.dart';
import 'mobile/drawer.dart' show largeText;
import 'mobile/input.dart' show nativeSelectionHere;
import 'mobile/metrics.dart';
import 'mobile/presentation.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'selection.dart';
import 'usage.dart' show formatAge;
import 'viewers/registry.dart' show HeaderTextButton;

// Comments on files (factory-research/annotations.md): the person selects a
// passage a viewer shows, comments on it, and sends the comments to an agent,
// which revises the file and replies. Threads live in the backend, by file;
// the app highlights their passages and shows them over the viewer: cards
// in its right margin on a wide pane, small markers on a narrow one. A
// selection gets a popover at it to comment from; a comment goes to its
// agent as soon as it's written.

/// Every thread on each file the backend has told this device about.
class ThreadStore extends ChangeNotifier {
  final _lists = <String, ThreadList>{};

  /// [path]'s threads, and the file's version they're anchored to; null
  /// until the backend has sent them.
  ThreadList? of(String path) => _lists[path];

  void apply(ThreadList list) {
    _lists[list.path] = list;
    notifyListeners();
  }

  /// One thread, new or changed (the answer to creating one).
  void put(Thread thread) {
    final list = _lists[thread.path];
    if (list == null) {
      _lists[thread.path] = ThreadList(
        path: thread.path,
        versionSha256: thread.versionSha256,
        threads: [thread],
      );
    } else {
      final threads = list.threads;
      final at = threads.indexWhere((old) => old.id == thread.id);
      if (at == -1) {
        threads.add(thread);
      } else {
        threads[at] = thread;
      }
    }
    notifyListeners();
  }
}

/// What the person selected in a viewer, for a new comment to be on: a
/// [Passage] of text, or a [Region] of an image.
sealed class Mark {
  const Mark();

  /// The thread kind it makes: "markdown", "text" or "image".
  String get kind;

  /// As W3C Web Annotation selectors.
  List<Selector> get selectors;

  /// Nothing to comment on (only whitespace, or a sliver).
  bool get isEmpty;
}

/// A region of an image, as fractions of its width and height from its
/// top left (0 to 1).
class Region extends Mark {
  const Region(this.rect);

  final Rect rect;

  @override
  String get kind => 'image';

  /// A spot rather than an area: a click, not a drag.
  bool get isPoint => rect.width == 0 && rect.height == 0;

  /// Too thin to mean anything: a sliver, not a spot or an area.
  @override
  bool get isEmpty => !isPoint && (rect.width < 0.005 || rect.height < 0.005);

  /// A media fragment, `xywh=percent:x,y,w,h`.
  @override
  List<Selector> get selectors => [
    Selector(
      json: jsonEncode({
        'type': 'FragmentSelector',
        'conformsTo': 'http://www.w3.org/TR/media-frags/',
        'value': fragment,
      }),
    ),
  ];

  String get fragment =>
      'xywh=percent:${_percent(rect.left)},${_percent(rect.top)},'
      '${_percent(rect.width)},${_percent(rect.height)}';

  static String _percent(double fraction) {
    final value = (fraction * 10000).round() / 100;
    return value == value.roundToDouble()
        ? value.round().toString()
        : value.toString();
  }

  @override
  bool operator ==(Object other) => other is Region && other.rect == rect;

  @override
  int get hashCode => rect.hashCode;
}

/// A moment of a video, or a stretch of it ([end] after [start]), in
/// seconds, and perhaps a region of the frame (fractions of it).
class Moment extends Mark {
  const Moment(this.start, {this.end, this.region});

  final double start;
  final double? end;
  final Rect? region;

  @override
  String get kind => 'video';

  @override
  bool get isEmpty => region != null && Region(region!).isEmpty;

  /// A media fragment: `t=31.2,45` (a range), `t=31.2` (a moment), with
  /// `&xywh=percent:x,y,w,h` for a region of the frame.
  @override
  List<Selector> get selectors {
    String seconds(double value) {
      final tenths = (value * 10).round() / 10;
      return tenths == tenths.roundToDouble()
          ? tenths.round().toString()
          : tenths.toString();
    }

    final end = this.end;
    final region = this.region;
    final time = end == null || end <= start
        ? 't=${seconds(start)}'
        : 't=${seconds(start)},${seconds(end)}';
    return [
      Selector(
        json: jsonEncode({
          'type': 'FragmentSelector',
          'conformsTo': 'http://www.w3.org/TR/media-frags/',
          'value': region == null ? time : '$time&${Region(region).fragment}',
        }),
      ),
    ];
  }

  /// "0:31–0:45", or "0:31" for a moment.
  String get label {
    final end = this.end;
    return end == null || end <= start
        ? formatTime(start)
        : '${formatTime(start)}–${formatTime(end)}';
  }

  /// Whether [seconds] is in it (a moment: within half a second of it).
  bool covers(double seconds) {
    final end = this.end;
    return end == null || end <= start
        ? (seconds - start).abs() <= 0.5
        : seconds >= start && seconds <= end;
  }

  @override
  bool operator ==(Object other) =>
      other is Moment &&
      other.start == start &&
      other.end == end &&
      other.region == region;

  @override
  int get hashCode => Object.hash(start, end, region);
}

/// [seconds] as a video's time: "0:07", "1:31", "1:02:05".
String formatTime(double seconds) {
  final whole = seconds.isFinite && seconds > 0 ? seconds.floor() : 0;
  final hours = whole ~/ 3600;
  final minutes = (whole % 3600) ~/ 60;
  final rest = (whole % 60).toString().padLeft(2, '0');
  return hours > 0
      ? '$hours:${minutes.toString().padLeft(2, '0')}:$rest'
      : '$minutes:$rest';
}

/// The moment or stretch a video's thread is on, from its media fragment
/// selector; null when it has none it can read.
Moment? momentOf(Thread thread) {
  for (final selector in thread.selectors) {
    try {
      final json = jsonDecode(selector.json);
      if (json is! Map || json['type'] != 'FragmentSelector') continue;
      double? start;
      double? end;
      Rect? region;
      for (final part in '${json['value']}'.split('&')) {
        if (part.startsWith('t=')) {
          final times = part.substring(2).replaceFirst('npt:', '').split(',');
          start = double.tryParse(times.first.isEmpty ? '0' : times.first);
          if (times.length > 1) end = double.tryParse(times[1]);
        } else if (part.startsWith('xywh=')) {
          region = _parseXywh(part);
        }
      }
      if (start != null) return Moment(start, end: end, region: region);
    } on FormatException {
      continue;
    }
  }
  return null;
}

/// A media fragment's region, `xywh=percent:x,y,w,h`, as fractions.
Rect? _parseXywh(String value) {
  final match = RegExp(
    r'^xywh=(percent:|pixel:)?([\d.]+),([\d.]+),([\d.]+),([\d.]+)$',
  ).firstMatch(value);
  if (match == null || match.group(1) != 'percent:') return null;
  double at(int group) => double.parse(match.group(group)!) / 100;
  return Rect.fromLTWH(at(2), at(3), at(4), at(5));
}

/// The region an image's thread is on, from its media fragment selector;
/// null when it has none it can read.
Rect? regionOf(Thread thread) {
  for (final selector in thread.selectors) {
    try {
      final json = jsonDecode(selector.json);
      if (json is! Map || json['type'] != 'FragmentSelector') continue;
      final region = _parseXywh('${json['value']}');
      if (region != null) return region;
    } on FormatException {
      continue;
    }
  }
  return null;
}

/// A passage of a viewer's text the person selected: what a new thread is
/// anchored to. [start] and [end] are UTF-16 offsets into the text the
/// viewer shows ([kind] "markdown": the rendered text; "text": the file).
class Passage extends Mark {
  factory Passage(String kind, String text, int a, int b) {
    final start = a < b ? a : b;
    final end = a < b ? b : a;
    const context = 32;
    return Passage._(
      kind: kind,
      start: start,
      end: end,
      exact: text.substring(start, end),
      prefix: text.substring(start - context < 0 ? 0 : start - context, start),
      suffix: text.substring(
        end,
        end + context > text.length ? text.length : end + context,
      ),
    );
  }

  const Passage._({
    required this.kind,
    required this.start,
    required this.end,
    required this.exact,
    required this.prefix,
    required this.suffix,
  });

  @override
  final String kind;
  final int start;
  final int end;
  final String exact;
  final String prefix;
  final String suffix;

  @override
  bool get isEmpty => exact.trim().isEmpty;

  /// A quote with its context, and where it is.
  @override
  List<Selector> get selectors => [
    Selector(
      json: jsonEncode({
        'type': 'TextQuoteSelector',
        'exact': exact,
        'prefix': prefix,
        'suffix': suffix,
      }),
    ),
    Selector(
      json: jsonEncode({
        'type': 'TextPositionSelector',
        'start': start,
        'end': end,
      }),
    ),
  ];

  @override
  bool operator ==(Object other) =>
      other is Passage &&
      other.kind == kind &&
      other.start == start &&
      other.end == end &&
      other.exact == exact;

  @override
  int get hashCode => Object.hash(kind, start, end, exact);
}

/// Where [anchor]'s passage is in [text]: its quote, at its place if it's
/// still there, else the match nearest it. Runs of whitespace match any
/// whitespace, since a viewer and the backend may join blocks differently.
/// Null when the quote isn't in [text].
TextRange? locatePassage(String text, TextAnchor anchor) {
  final exact = anchor.exact;
  if (exact.trim().isEmpty) return null;
  if (anchor.start >= 0 &&
      anchor.end <= text.length &&
      anchor.start <= anchor.end &&
      text.substring(anchor.start, anchor.end) == exact) {
    return TextRange(start: anchor.start, end: anchor.end);
  }
  final words = exact.trim().split(RegExp(r'\s+')).map(RegExp.escape);
  final pattern = RegExp(words.join(r'\s+'));
  TextRange? best;
  var distance = 0;
  for (final match in pattern.allMatches(text)) {
    final away = (match.start - anchor.start).abs();
    if (best == null || away < distance) {
      best = TextRange(start: match.start, end: match.end);
      distance = away;
    }
  }
  return best;
}

/// The anchor [thread] has in a viewer showing [kind]'s text.
TextAnchor? anchorIn(Thread thread, String kind) {
  if (thread.anchor == AnchorState.ANCHOR_STATE_DETACHED) return null;
  final anchor = kind == 'markdown' ? thread.rendered : thread.source;
  return anchor.exact.isEmpty ? null : anchor;
}

/// Whether [thread] has a comment of the person's not sent yet.
bool hasDraft(Thread thread) => thread.comments.any((c) => c.draft);

/// The viewer side of comments, for one file in one viewer: the threads on
/// it, where their passages are in the text it shows (or their regions of
/// the image), the person's selection, and what's being written. The
/// viewer tells it what it shows, how to find a y for an offset ([yOf]) or
/// a region ([yOfRegion]) and how to scroll to one ([reveal],
/// [revealRegion]), and calls [relayout] when what it shows moves.
class CommentsController extends ChangeNotifier {
  CommentsController({required this.backend, required this.path}) {
    backend.threads.addListener(_threadsChanged);
    backend.connected.addListener(_reconnected);
  }

  final Backend backend;
  final String path;

  /// "markdown" while the Markdown shows formatted, "image" for an image,
  /// else "text".
  String _kind = 'text';
  String get kind => _kind;

  /// The text the viewer shows, in [kind]'s space.
  String _text = '';

  /// The file's version the viewer shows (FileContent.sha256).
  String _version = '';

  /// Where each thread's passage is in the text shown, by thread id.
  var located = <String, TextRange>{};

  /// Each image thread's region, as fractions of the image, by thread id.
  var regions = <String, Rect>{};

  /// Each video thread's moment or stretch, by thread id.
  var moments = <String, Moment>{};

  /// Plays the video from a thread's moment, when one is picked.
  void Function(Moment moment)? revealMoment;

  /// The image shown, for the cards' thumbnails of their regions.
  ui.Image? get image => _image;
  ui.Image? _image;
  set image(ui.Image? image) {
    if (identical(image, _image)) return;
    _image = image;
    notifyListeners();
  }

  /// The person's selection, while there is one.
  final selection = ValueNotifier<Mark?>(null);

  /// What a comment is being written on, while it is.
  Mark? composing;

  /// What's being written on [composing] suggests a change to it.
  var suggesting = false;

  /// The thread a reply is being written to, or a comment edited in.
  String? replyingTo;
  String? editing;

  /// The thread the person picked: its card and passage stand out.
  String? focused;

  /// Resolved threads show too.
  var showResolved = false;

  /// On a narrow pane, the sheet listing them is up.
  var sheetOpen = false;

  /// The pane is too narrow for cards beside the text: they're markers,
  /// each opening its card at its text, and the header's Comments button
  /// opens the sheet listing them all.
  final narrow = ValueNotifier<bool>(false);

  /// Why the last request failed, till the next one.
  String? get error => failure.value;
  set error(String? error) => failure.value = error;

  /// [error], to listen to (a phone's editor shows it under its field).
  final failure = ValueNotifier<String?>(null);

  /// Where an offset of the text shown is, as a y in the viewer's box;
  /// null when it isn't laid out.
  double? Function(int offset)? yOf;

  /// Scrolls the viewer so an offset is in view.
  void Function(int offset)? reveal;

  /// Where a region of the image is, as the y of its top in the viewer's
  /// box, and how to bring it into view.
  double? Function(Rect region)? yOfRegion;
  void Function(Rect region)? revealRegion;

  /// Where a passage ([start] to [end] of the text shown), a region of the
  /// image or a moment of the video is on screen, in global coordinates,
  /// for a popover to sit by; null when it isn't laid out.
  Rect? Function(int start, int end)? rectOfRange;
  Rect? Function(Rect region)? rectOfRegion;
  Rect? Function(Moment moment)? rectOfMoment;

  /// Scrolls the viewer by [dy], for a wheel over the cards.
  void Function(double dy)? scrollBy;

  /// Where [mark] is on screen (global), if the viewer can say.
  Rect? rectOf(Mark mark) => switch (mark) {
    Passage(:final start, :final end) => rectOfRange?.call(start, end),
    Region(:final rect) => rectOfRegion?.call(rect),
    Moment() => rectOfMoment?.call(mark),
  };

  /// Where [threadId]'s passage, region or moment is on screen (global).
  Rect? rectOfThread(String threadId) {
    final range = located[threadId];
    if (range != null) return rectOfRange?.call(range.start, range.end);
    final region = regions[threadId];
    if (region != null) return rectOfRegion?.call(region);
    final moment = moments[threadId];
    if (moment != null) return rectOfMoment?.call(moment);
    return null;
  }

  /// Whether [threadId]'s passage or region is in what's shown.
  bool isPlaced(String threadId) =>
      located.containsKey(threadId) ||
      regions.containsKey(threadId) ||
      moments.containsKey(threadId);

  /// Where [threadId]'s card goes: level with its passage or region.
  double? yOfThread(String threadId) {
    final range = located[threadId];
    if (range != null) return yOf?.call(range.start);
    final region = regions[threadId];
    if (region != null) return yOfRegion?.call(region);
    return null;
  }

  /// Where a card for [mark] goes.
  double? yOfMark(Mark mark) => switch (mark) {
    Passage(:final start) => yOf?.call(start),
    Region(:final rect) => yOfRegion?.call(rect),
    // A video's cards go down the margin in time order.
    Moment() => null,
  };

  /// The number on each pin, by thread id: the comments on a spot or an
  /// area of an image or a video's frame, from 1 in reading order.
  Map<String, int> get pins {
    final numbers = <String, int>{};
    for (final thread in inOrder(shown)) {
      if (regions.containsKey(thread.id) ||
          moments[thread.id]?.region != null) {
        numbers[thread.id] = numbers.length + 1;
      }
    }
    return numbers;
  }

  /// [threads] in reading order, those not placed first.
  List<Thread> inOrder(List<Thread> threads) =>
      [...threads]..sort((a, b) => orderOf(a.id).compareTo(orderOf(b.id)));

  /// [threadId]'s place in reading order: its passage's start, or its
  /// region's top, then left.
  double orderOf(String threadId) {
    final range = located[threadId];
    if (range != null) return range.start.toDouble();
    final region = regions[threadId];
    if (region != null) return region.top * 1000 + region.left;
    final moment = moments[threadId];
    if (moment != null) return moment.start;
    return -1;
  }

  /// Fires when what the viewer shows moved (scrolled, laid out again).
  final layout = ChangeNotifier();

  void relayout() => layout.notifyListeners();

  @override
  void dispose() {
    backend.threads.removeListener(_threadsChanged);
    backend.connected.removeListener(_reconnected);
    selection.dispose();
    narrow.dispose();
    failure.dispose();
    layout.dispose();
    super.dispose();
  }

  /// Whether the viewer has shown the file's text yet: only then are its
  /// threads asked for, so a file that can't take comments asks nothing.
  var _shown = false;

  /// Asks for the threads again (first shown, after a reconnect).
  void refresh() {
    if (!_shown || !backend.connected.value) return;
    backend.listThreads(path).catchError((Object error) {});
  }

  void _reconnected() {
    if (backend.connected.value) refresh();
  }

  /// What the viewer shows now.
  void show({
    required String kind,
    required String text,
    required String version,
  }) {
    if (!_shown) {
      _shown = true;
      refresh();
    }
    if (kind == _kind && text == _text && version == _version) return;
    final kindChanged = kind != _kind;
    _kind = kind;
    _text = text;
    _version = version;
    if (kindChanged) {
      selection.value = null;
      composing = null;
    }
    _locate();
    notifyListeners();
  }

  void _threadsChanged() {
    _locate();
    notifyListeners();
  }

  /// Every thread on the file, as the backend last sent them.
  List<Thread> get all => backend.threads.of(path)?.threads ?? const [];

  /// The threads to show: open ones, and resolved ones when asked.
  List<Thread> get shown => [
    for (final thread in all)
      if (showResolved || thread.status != ThreadStatus.THREAD_STATUS_RESOLVED)
        thread,
  ];

  /// Threads whose passage isn't in the text shown.
  List<Thread> get detached => [
    for (final thread in shown)
      if (!isPlaced(thread.id)) thread,
  ];

  int get resolvedCount => all
      .where((thread) => thread.status == ThreadStatus.THREAD_STATUS_RESOLVED)
      .length;

  void _locate() {
    final list = backend.threads.of(path);
    if (_kind == 'video') {
      // A moment stays where it was made, on any version of the video.
      located = {};
      regions = {};
      moments = {
        for (final thread in list?.threads ?? <Thread>[])
          thread.id: ?momentOf(thread),
      };
      return;
    }
    moments = {};
    if (_kind == 'image') {
      // A region stays where it was made, on any version of the image.
      located = {};
      regions = {
        for (final thread in list?.threads ?? <Thread>[])
          thread.id: ?regionOf(thread),
      };
      return;
    }
    regions = {};
    final found = <String, TextRange>{};
    // Positions are for the version they were anchored to; the backend
    // sends them again for the new one after the file changes.
    if (list != null && (list.versionSha256 == _version || _version.isEmpty)) {
      for (final thread in list.threads) {
        final anchor = anchorIn(thread, _kind);
        if (anchor == null) continue;
        final range = locatePassage(_text, anchor);
        if (range != null) found[thread.id] = range;
      }
    }
    located = found;
  }

  /// The agent comments go to unless the person picks another: the one
  /// whose pane shows the file.
  String get defaultAgent =>
      backend.workspace.workspace.selectedAgentId.isNotEmpty
      ? backend.workspace.workspace.selectedAgentId
      : (backend.workspace.workspace.agents.firstOrNull?.id ?? '');

  /// Drafts on this file, by the agent they're addressed to.
  Map<String, int> get drafts {
    final counts = <String, int>{};
    for (final thread in all) {
      if (hasDraft(thread)) {
        counts[thread.agentId] = (counts[thread.agentId] ?? 0) + 1;
      }
    }
    return counts;
  }

  void focus(String? threadId) {
    if (focused == threadId) return;
    focused = threadId;
    final range = threadId == null ? null : located[threadId];
    if (range != null) reveal?.call(range.start);
    final region = threadId == null ? null : regions[threadId];
    if (region != null) revealRegion?.call(region);
    final moment = threadId == null ? null : moments[threadId];
    if (moment != null) revealMoment?.call(moment);
    notifyListeners();
  }

  /// A click away from the picked thread lets it go, unless something's
  /// being written in it.
  void letGo() {
    if (replyingTo != null || editing != null) return;
    focus(null);
  }

  /// Starts a comment on the selection, or a suggested change to it.
  void comment({bool suggest = false}) {
    final mark = selection.value;
    if (mark == null || mark.isEmpty) return;
    composing = mark;
    // Only text can be replaced.
    suggesting = suggest && mark is Passage;
    replyingTo = null;
    editing = null;
    focused = null;
    notifyListeners();
  }

  /// Lets go of the comment being written. A region picked for it goes
  /// too, so it isn't left outlined; text stays selected.
  void cancel() {
    if (selection.value is Region) selection.value = null;
    composing = null;
    suggesting = false;
    replyingTo = null;
    editing = null;
    notifyListeners();
  }

  void startReply(String threadId) {
    replyingTo = threadId;
    editing = null;
    composing = null;
    focused = threadId;
    notifyListeners();
  }

  void startEdit(String commentId, String threadId) {
    editing = commentId;
    replyingTo = null;
    composing = null;
    focused = threadId;
    notifyListeners();
  }

  void toggleResolved() {
    showResolved = !showResolved;
    notifyListeners();
  }

  void openSheet(bool open) {
    if (sheetOpen == open) return;
    sheetOpen = open;
    if (!open) cancel();
    notifyListeners();
  }

  Future<bool> _run(Future<void> Function() request) async {
    try {
      await request();
      error = null;
      notifyListeners();
      return true;
    } on RequestFailure catch (failure) {
      error = failure.message;
      notifyListeners();
      return false;
    }
  }

  /// Sends the comment being written to its agent, or with [sendNow] false
  /// keeps it as a draft; whether it went.
  Future<bool> submit(
    String body, {
    required bool sendNow,
    String? replacement,
  }) async {
    final text = body.trim();
    final suggestion = suggesting ? replacement : null;
    if (text.isEmpty && suggestion == null) return false;
    final mark = composing;
    final reply = replyingTo;
    final edited = editing;
    final ok = await _run(() async {
      if (edited != null) {
        await backend.editComment(edited, text);
      } else if (reply != null) {
        await backend.replyThread(reply, text, sendNow: sendNow);
      } else if (mark != null) {
        final thread = await backend.createThread(
          CreateThread(
            agentId: defaultAgent,
            path: path,
            kind: mark.kind,
            versionSha256: _version,
            selectors: mark.selectors,
            body: text,
            sendNow: sendNow,
            suggestion: suggestion,
          ),
        );
        // Its card stands out in the margin; a narrow pane's marker
        // doesn't open it.
        if (!narrow.value) focused = thread.id;
      }
    });
    if (ok) {
      composing = null;
      suggesting = false;
      replyingTo = null;
      editing = null;
      selection.value = null;
      notifyListeners();
    }
    return ok;
  }

  Future<void> resolve(Thread thread, {required bool resolved}) =>
      _run(() => backend.setThreadStatus(thread.id, resolved: resolved));

  Future<void> readdress(Thread thread, String agentId) =>
      _run(() => backend.readdressThread(thread.id, agentId));

  Future<void> delete(ThreadComment comment) =>
      _run(() => backend.deleteComment(comment.id));

  /// Accepts [thread]'s suggested change, which Claktory makes in the
  /// file, or rejects it. The person's own, never sent, is taken back
  /// instead: deleting its comments deletes it. [reply] does
  /// neither: [comment] goes into the thread, which stays open. Whether
  /// it went.
  Future<bool> decide(
    Thread thread, {
    required bool accept,
    String comment = '',
    bool reply = false,
  }) {
    final own = thread.suggestion.authorAgentId.isEmpty;
    if (!accept && own && thread.comments.every((c) => c.draft)) {
      return _run(() async {
        for (final comment in thread.comments) {
          await backend.deleteComment(comment.id);
        }
      });
    }
    return _run(
      () => backend.decideSuggestion(
        thread.id,
        accept: accept,
        comment: comment,
        reply: reply,
      ),
    );
  }

  Future<void> send(String agentId) =>
      _run(() => backend.sendComments(agentId, path));

  /// Anchors a detached thread to the selection instead.
  Future<void> reattach(Thread thread) async {
    final mark = selection.value;
    if (mark == null || mark.isEmpty) return;
    final ok = await _run(
      () => backend.reattachThread(
        ReattachThread(
          threadId: thread.id,
          kind: mark.kind,
          versionSha256: _version,
          selectors: mark.selectors,
        ),
      ),
    );
    if (ok) selection.value = null;
  }
}

/// The highlight of a thread's passage: its agent's color, stronger while
/// it's the one picked.
Color passageColor(String agentColorName, {required bool focused}) =>
    agentColor(agentColorName).withValues(alpha: focused ? 0.40 : 0.20);

/// [span] with the ranges in [marks] (offsets into its plain text, from
/// [base]) painted behind in their colors. The text and its layout don't
/// change, only backgrounds.
TextSpan markSpan(
  TextSpan span,
  List<(int start, int end, Color color)> marks, {
  int base = 0,
}) {
  if (marks.isEmpty) return span;
  var offset = base;
  InlineSpan mark(InlineSpan piece) {
    if (piece is! TextSpan) {
      offset += 1;
      return piece;
    }
    final parts = <InlineSpan>[];
    final text = piece.text;
    if (text != null && text.isNotEmpty) {
      final start = offset;
      final end = offset + text.length;
      // Cut points: every mark edge inside this text.
      final cuts = <int>{start, end};
      for (final (from, to, _) in marks) {
        if (from > start && from < end) cuts.add(from);
        if (to > start && to < end) cuts.add(to);
      }
      final sorted = cuts.toList()..sort();
      for (var i = 0; i + 1 < sorted.length; i++) {
        final from = sorted[i];
        final to = sorted[i + 1];
        Color? color;
        for (final (markFrom, markTo, markColor) in marks) {
          if (markFrom <= from && markTo >= to) {
            color = color == null
                ? markColor
                : Color.alphaBlend(markColor, color);
          }
        }
        final cut = text.substring(from - start, to - start);
        parts.add(
          // A link stays one: the hit test finds the piece, not its parent.
          TextSpan(
            text: cut,
            style: color == null ? null : TextStyle(backgroundColor: color),
            recognizer: piece.recognizer,
            mouseCursor: piece.mouseCursor,
            onEnter: piece.onEnter,
            onExit: piece.onExit,
          ),
        );
      }
      offset = end;
    }
    final children = [
      for (final child in piece.children ?? const []) mark(child),
    ];
    return TextSpan(
      style: piece.style,
      recognizer: piece.recognizer,
      mouseCursor: piece.mouseCursor,
      onEnter: piece.onEnter,
      onExit: piece.onExit,
      semanticsLabel: piece.semanticsLabel,
      children: [...parts, ...children],
    );
  }

  return mark(span) as TextSpan;
}

/// The comments over a viewer. Nothing is set aside for them until there
/// are some: then on a wide pane their cards sit in a margin at its right,
/// each level with its text and reaching a little over the document's own
/// edge; on a narrow one (and the phone) a small marker per comment, which
/// opens its card at its text. A selection gets a popover at it to comment
/// from, and the comment is written there. [child] is the viewer's body.
class Commented extends StatefulWidget {
  const Commented({
    super.key,
    required this.controller,
    required this.child,
    this.insetInside = false,
  });

  final CommentsController controller;
  final Widget child;

  /// The child makes the room for the cards itself, inside what it
  /// scrolls ([CommentsInset]), so the text under them still scrolls and
  /// selects; else it's padded.
  final bool insetInside;

  /// A card's width, how far in from the pane's right edge it sits, and how
  /// far over the document's edge (its own padding) it may reach.
  static const cardWidth = 240.0;
  static const edge = Space.s;
  static const overlap = Space.l;

  /// The room the cards take from the document.
  static const marginInset = cardWidth + edge - overlap;

  /// The narrowest the document gets beside the cards: narrower panes
  /// have markers instead.
  static const minReading = 480.0;
  static const wideFrom = minReading + marginInset;

  /// The markers' strip on a narrow pane.
  static const markerWidth = 20.0;

  /// A popover's width: writing a comment, and a comment's card.
  static const popoverWidth = 320.0;

  @override
  State<Commented> createState() => _CommentedState();
}

class _CommentedState extends State<Commented> {
  final _stack = GlobalKey();

  /// Whether the latest press in it was a finger's: a selection made by
  /// touch has the system's own menu, Comment and Suggest in it.
  final _touch = _Touched();

  @override
  Widget build(BuildContext context) {
    final controller = widget.controller;
    final phone = isPhoneLayout(context);
    return LayoutBuilder(
      builder: (context, constraints) {
        // A phone has its sheet and markers whichever way it's held.
        final wide = !phone && constraints.maxWidth >= Commented.wideFrom;
        if (controller.narrow.value == wide) {
          // Told after the frame: the header is built already.
          WidgetsBinding.instance.addPostFrameCallback((_) {
            controller.narrow.value = !wide;
            if (wide) controller.openSheet(false);
          });
        }
        return ListenableBuilder(
          listenable: controller,
          builder: (context, _) {
            final shown = controller.shown;
            // A video's comments are on its timeline already.
            final markers = !wide && controller.kind != 'video';
            final inset = shown.isEmpty
                ? 0.0
                : wide
                ? Commented.marginInset
                : markers
                ? Commented.markerWidth
                : 0.0;
            return Listener(
              onPointerDown: (event) =>
                  _touch.value = event.kind == PointerDeviceKind.touch,
              child: Stack(
                key: _stack,
                children: [
                  Positioned.fill(
                    child: widget.insetInside
                        ? CommentsInset(right: inset, child: widget.child)
                        : Padding(
                            padding: EdgeInsets.only(right: inset),
                            child: widget.child,
                          ),
                  ),
                  if (shown.isNotEmpty && wide)
                    Positioned(
                      top: 0,
                      bottom: 0,
                      right: Commented.edge,
                      width: Commented.cardWidth,
                      child: _Margin(controller: controller),
                    )
                  else if (shown.isNotEmpty && markers)
                    Positioned(
                      top: 0,
                      bottom: 0,
                      right: 0,
                      // A finger's width to tap, reaching over the text's
                      // edge; the dots stay in their strip.
                      width: phone ? Touch.target : Commented.markerWidth,
                      child: _Markers(controller: controller),
                    ),
                  Positioned.fill(
                    child: _Popovers(
                      controller: controller,
                      stack: _stack,
                      wide: wide,
                      right: inset,
                      touch: _touch,
                    ),
                  ),
                  Positioned(
                    right: Space.m,
                    bottom: Space.m,
                    child: _Footer(controller: controller),
                  ),
                  if (phone)
                    _PhoneComments(controller: controller)
                  else if (!wide)
                    Positioned.fill(child: _Sheet(controller: controller)),
                ],
              ),
            );
          },
        );
      },
    );
  }
}

/// How much room at its right a viewer inside [Commented] leaves for the
/// comments, when it makes the room itself ([Commented.insetInside]).
class CommentsInset extends InheritedWidget {
  const CommentsInset({super.key, required this.right, required super.child});

  final double right;

  static double of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<CommentsInset>()?.right ?? 0;

  @override
  bool updateShouldNotify(CommentsInset old) => old.right != right;
}

/// The header's control for the comments. On a wide pane, while some are
/// resolved: "Show 2 resolved" or "Hide resolved". On a narrow one: how
/// many there are; it opens the sheet listing them.
class CommentsButton extends StatelessWidget {
  const CommentsButton({super.key, required this.controller});

  final CommentsController controller;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([controller, controller.narrow]),
    builder: (context, _) {
      if (!controller.narrow.value) {
        final resolved = controller.resolvedCount;
        if (resolved == 0) return const SizedBox.shrink();
        return HeaderTextButton(
          controller.showResolved ? 'Hide resolved' : 'Show $resolved resolved',
          controller.toggleResolved,
        );
      }
      final count = controller.shown.length;
      if (isPhoneLayout(context)) {
        return _PhoneCommentsButton(controller: controller, count: count);
      }
      return GlyphButton(
        label: count == 0 ? 'Comments' : '$count comments',
        active: controller.sheetOpen,
        onTap: () => controller.openSheet(!controller.sheetOpen),
        glyph: (color) => ExtensionGlyph(
          icon: 'message-square',
          name: 'Comments',
          color: color,
        ),
      );
    },
  );
}

/// What floats over the viewer at a place in it: the popover at the
/// selection (Comment, Suggest), the comment being written there, and on a
/// narrow pane the picked comment's card at its text.
class _Popovers extends StatelessWidget {
  const _Popovers({
    required this.controller,
    required this.stack,
    required this.wide,
    required this.right,
    required this.touch,
  });

  /// Whether the latest press was a finger's ([_CommentedState._touch]).
  final _Touched touch;

  final CommentsController controller;
  final GlobalKey stack;
  final bool wide;

  /// What the cards or markers take at the right.
  final double right;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([
      controller,
      controller.layout,
      controller.selection,
    ]),
    builder: (context, _) {
      // A phone writes and reads them in sheets ([_PhoneComments]).
      if (controller.sheetOpen || isPhoneLayout(context)) {
        return const SizedBox.shrink();
      }
      final box = stack.currentContext?.findRenderObject();
      if (box is! RenderBox || !box.hasSize) {
        // Laid out after this frame: placed then.
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (stack.currentContext != null) controller.relayout();
        });
        return const SizedBox.shrink();
      }
      Rect? local(Rect? global) => global == null
          ? null
          : Rect.fromPoints(
              box.globalToLocal(global.topLeft),
              box.globalToLocal(global.bottomRight),
            );
      final size = box.size;
      // Nothing to sit by: low in the middle.
      final fallback = Rect.fromLTWH(
        size.width / 2 - Commented.popoverWidth / 2,
        size.height - Space.l,
        0,
        0,
      );
      final reduced = MediaQuery.disableAnimationsOf(context);
      final composing = controller.composing;
      if (composing != null) {
        return _Popover(
          anchor: local(controller.rectOf(composing)) ?? fallback,
          // Over a video's timeline, which is at the bottom.
          above: composing is Moment,
          child: Arrival(
            key: ValueKey(('composing', composing, controller.suggesting)),
            animate: !reduced,
            child: SizedBox(
              width: Commented.popoverWidth,
              child: _ComposerCard(controller: controller, mark: composing),
            ),
          ),
        );
      }
      final mark = controller.selection.value;
      // Selected with a finger where text selects the platform's way (an
      // iPad), its own menu has Comment and Suggest already: none float
      // over it too. A trackpad's selection has no menu, so they do.
      final menu = touch.value && nativeSelectionHere;
      if (mark != null && !mark.isEmpty && !menu) {
        return _Popover(
          anchor: local(controller.rectOf(mark)) ?? fallback,
          above: true,
          child: Arrival(
            key: const ValueKey('selection'),
            animate: !reduced,
            child: _SelectionActions(controller: controller, mark: mark),
          ),
        );
      }
      final focused = controller.focused;
      if (!wide && focused != null) {
        final thread = controller.shown
            .where((thread) => thread.id == focused)
            .firstOrNull;
        if (thread == null) return const SizedBox.shrink();
        return _Popover(
          anchor:
              local(controller.rectOfThread(focused)) ??
              Rect.fromLTWH(size.width, Space.s, 0, 0),
          alignRight: right + Space.xs,
          child: Arrival(
            key: ValueKey(('thread', focused)),
            animate: !reduced,
            child: SizedBox(
              width: Commented.popoverWidth,
              child: _ThreadCard(controller: controller, thread: thread),
            ),
          ),
        );
      }
      return const SizedBox.shrink();
    },
  );
}

/// Places [child] by [anchor] (in the box's coordinates): below it, or
/// above it with [above] or when there's no room below; at its left, or
/// [alignRight] in from the box's right. Always kept inside the box, and
/// scrolling when it's taller.
class _Popover extends StatelessWidget {
  const _Popover({
    required this.anchor,
    required this.child,
    this.above = false,
    this.alignRight,
  });

  final Rect anchor;
  final Widget child;
  final bool above;
  final double? alignRight;

  @override
  Widget build(BuildContext context) => CustomSingleChildLayout(
    delegate: _PopoverLayout(
      anchor: anchor,
      above: above,
      alignRight: alignRight,
    ),
    child: SingleChildScrollView(child: child),
  );
}

class _PopoverLayout extends SingleChildLayoutDelegate {
  _PopoverLayout({
    required this.anchor,
    required this.above,
    required this.alignRight,
  });

  final Rect anchor;
  final bool above;
  final double? alignRight;

  /// Between the popover and what it's at, and the box's edges.
  static const gap = Space.xs + 2;
  static const margin = Space.s;

  @override
  BoxConstraints getConstraintsForChild(BoxConstraints constraints) =>
      BoxConstraints(
        maxWidth: (constraints.maxWidth - 2 * margin).clamp(0, double.infinity),
        maxHeight: (constraints.maxHeight - 2 * margin).clamp(
          0,
          double.infinity,
        ),
      );

  @override
  Offset getPositionForChild(Size size, Size child) {
    final right = alignRight;
    final x = right != null ? size.width - right - child.width : anchor.left;
    final below = anchor.bottom + gap;
    final over = anchor.top - gap - child.height;
    final fitsBelow = below + child.height <= size.height - margin;
    final fitsOver = over >= margin;
    final y = above
        ? (fitsOver || !fitsBelow ? over : below)
        : (fitsBelow || !fitsOver ? below : over);
    double keep(double value, double extent, double room) {
      final most = room - extent - margin;
      return most < margin ? margin : value.clamp(margin, most);
    }

    return Offset(
      keep(x, child.width, size.width),
      keep(y, child.height, size.height),
    );
  }

  @override
  bool shouldRelayout(_PopoverLayout old) =>
      old.anchor != anchor ||
      old.above != above ||
      old.alignRight != alignRight;
}

/// Whether the latest press in a [Commented] was a finger's. Read as the
/// selection changes, after the press, so nothing listens to it.
class _Touched {
  bool value = false;
}

/// At the selection: a primary Comment, and Suggest for text.
class _SelectionActions extends StatelessWidget {
  const _SelectionActions({required this.controller, required this.mark});

  final CommentsController controller;
  final Mark mark;

  @override
  Widget build(BuildContext context) => Unselectable(
    child: _Floating(
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Button(
            label: 'Comment',
            primary: true,
            onPressed: controller.comment,
          ),
          if (mark is Passage) ...[
            const SizedBox(width: Space.xs),
            Button(
              label: 'Suggest',
              onPressed: () => controller.comment(suggest: true),
            ),
          ],
        ],
      ),
    ),
  );
}

class _Floating extends StatelessWidget {
  const _Floating({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Container(
      padding: const EdgeInsets.all(Space.xs),
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
        boxShadow: floatingShadow,
      ),
      child: child,
    );
  }
}

/// The wide pane's margin: each card level with its text, the rest from
/// the top. A wheel over a card scrolls the document.
class _Margin extends StatelessWidget {
  const _Margin({required this.controller});

  final CommentsController controller;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: Listenable.merge([controller, controller.layout]),
      builder: (context, _) {
        final threads = controller.inOrder(controller.shown);
        final focused = threads.indexWhere((t) => t.id == controller.focused);
        return Listener(
          onPointerSignal: (event) {
            if (event is PointerScrollEvent) {
              controller.scrollBy?.call(event.scrollDelta.dy);
            }
          },
          child: ClipRect(
            child: CustomMultiChildLayout(
              delegate: _Stacked([
                for (final t in threads) controller.yOfThread(t.id),
              ], focused: focused == -1 ? null : focused),
              children: [
                for (var i = 0; i < threads.length; i++)
                  LayoutId(
                    id: i,
                    child: _ThreadCard(
                      controller: controller,
                      thread: threads[i],
                    ),
                  ),
              ],
            ),
          ),
        );
      },
    );
  }
}

/// Things down a strip, each level with what it's about ([wanted], a y)
/// unless it would overlap the one before; those with nowhere to be
/// (null) first, from the top. The [focused] one keeps its place exactly,
/// and those above it move up to make room.
class _Stacked extends MultiChildLayoutDelegate {
  _Stacked(this.wanted, {this.focused, this.gap = Space.s});

  final List<double?> wanted;
  final int? focused;
  final double gap;

  @override
  void performLayout(Size size) {
    final heights = [
      for (var i = 0; i < wanted.length; i++)
        layoutChild(i, BoxConstraints(maxWidth: size.width)).height,
    ];
    var top = Space.s;
    final placed = <int>[];
    for (var i = 0; i < wanted.length; i++) {
      if (wanted[i] == null) {
        positionChild(i, Offset(0, top));
        top += heights[i] + gap;
      } else {
        placed.add(i);
      }
    }
    final unplaced = placed.length < wanted.length;
    placed.sort((a, b) => wanted[a]!.compareTo(wanted[b]!));
    final pivot = focused == null ? -1 : placed.indexOf(focused!);
    // Those scrolled above stay above: only the unplaced hold the top.
    var next = unplaced ? top : double.negativeInfinity;
    var from = 0;
    if (pivot != -1) {
      final i = placed[pivot];
      final y = wanted[i]! > next ? wanted[i]! : next;
      positionChild(i, Offset(0, y));
      var limit = y - gap;
      for (var j = pivot - 1; j >= 0; j--) {
        final k = placed[j];
        final at = wanted[k]! + heights[k] < limit
            ? wanted[k]!
            : limit - heights[k];
        positionChild(k, Offset(0, at));
        limit = at - gap;
      }
      next = y + heights[i] + gap;
      from = pivot + 1;
    }
    for (var j = from; j < placed.length; j++) {
      final i = placed[j];
      final y = wanted[i]! > next ? wanted[i]! : next;
      positionChild(i, Offset(0, y));
      next = y + heights[i] + gap;
    }
  }

  @override
  bool shouldRelayout(_Stacked old) =>
      !listEquals(old.wanted, wanted) || old.focused != focused;
}

/// A narrow pane's markers: a dot in the agent's color level with each
/// comment's text; a click opens its card there.
class _Markers extends StatelessWidget {
  const _Markers({required this.controller});

  final CommentsController controller;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([controller, controller.layout]),
    builder: (context, _) {
      final threads = controller.inOrder(controller.shown);
      final focused = threads.indexWhere((t) => t.id == controller.focused);
      // A phone's are a finger's size, centered on their line.
      final lift = isPhoneLayout(context)
          ? (Touch.target - Commented.markerWidth) / 2
          : 0.0;
      return Listener(
        onPointerSignal: (event) {
          if (event is PointerScrollEvent) {
            controller.scrollBy?.call(event.scrollDelta.dy);
          }
        },
        child: ClipRect(
          child: CustomMultiChildLayout(
            delegate: _Stacked(
              [
                for (final t in threads)
                  if (controller.yOfThread(t.id) case final y?)
                    y - lift
                  else
                    null,
              ],
              focused: focused == -1 ? null : focused,
              gap: 0,
            ),
            children: [
              for (var i = 0; i < threads.length; i++)
                LayoutId(
                  id: i,
                  child: _Marker(controller: controller, thread: threads[i]),
                ),
            ],
          ),
        ),
      );
    },
  );
}

class _Marker extends StatelessWidget {
  const _Marker({required this.controller, required this.thread});

  final CommentsController controller;
  final Thread thread;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final focused = controller.focused == thread.id;
    final agent = _agent(controller.backend, thread.agentId);
    final resolved = thread.status == ThreadStatus.THREAD_STATUS_RESOLVED;
    // On a phone, a finger's size, the dot at its end in the strip.
    final phone = isPhoneLayout(context);
    return Semantics(
      container: true,
      button: true,
      label: 'Comment for ${agent?.name ?? thread.agentId}',
      onTap: () => controller.focus(focused ? null : thread.id),
      excludeSemantics: true,
      child: Pressable(
        onTap: () => controller.focus(focused ? null : thread.id),
        builder: (context, hovered) {
          final dot = Container(
            width: Commented.markerWidth,
            height: Commented.markerWidth,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: focused
                  ? palette.selected
                  : hovered
                  ? palette.hover
                  : const Color(0x00000000),
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            child: Opacity(
              opacity: resolved ? 0.6 : 1,
              child: Dot(
                color: agentColor(agent?.color ?? ''),
                size: focused ? 10 : 8,
              ),
            ),
          );
          if (!phone) return dot;
          return SizedBox.square(
            dimension: Touch.target,
            child: Align(alignment: Alignment.centerRight, child: dot),
          );
        },
      ),
    );
  }
}

/// At the pane's bottom right, floating, only while there's something:
/// why the last request failed, and comments kept from before they were
/// sent as soon as written, to send now ("Send 3 comments to June").
class _Footer extends StatelessWidget {
  const _Footer({required this.controller});

  final CommentsController controller;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: controller,
    builder: (context, _) {
      final palette = Palette.of(context);
      final error = controller.error;
      final drafts = controller.drafts;
      if (controller.sheetOpen || (error == null && drafts.isEmpty)) {
        return const SizedBox.shrink();
      }
      return Unselectable(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: Commented.cardWidth),
          child: _Floating(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (error != null)
                  Padding(
                    padding: const EdgeInsets.all(Space.xs),
                    child: Text(
                      error,
                      style: TextStyles.caption.copyWith(color: palette.danger),
                    ),
                  ),
                for (final MapEntry(key: agentId, value: count)
                    in drafts.entries)
                  Padding(
                    padding: const EdgeInsets.all(Space.xs / 2),
                    child: Button(
                      label:
                          'Send $count ${count == 1 ? 'comment' : 'comments'} '
                          'to ${_agentName(controller.backend, agentId)}',
                      primary: true,
                      onPressed: () => controller.send(agentId),
                    ),
                  ),
              ],
            ),
          ),
        ),
      );
    },
  );
}

/// Above the cards, when there's something: how many are resolved, to show
/// them, and why a request failed.
class _MarginHeader extends StatelessWidget {
  const _MarginHeader({required this.controller});

  final CommentsController controller;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final resolved = controller.resolvedCount;
    final error = controller.error;
    if (resolved == 0 && error == null) return const SizedBox.shrink();
    return Unselectable(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(Space.m, Space.s, Space.m, 0),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (resolved > 0)
              Pressable(
                onTap: controller.toggleResolved,
                builder: (context, hovered) => Text(
                  controller.showResolved
                      ? 'Hide resolved'
                      : 'Show $resolved resolved',
                  style: TextStyles.caption.copyWith(
                    color: hovered ? palette.text : palette.textMuted,
                  ),
                ),
              ),
            if (error != null)
              Text(
                error,
                style: TextStyles.caption.copyWith(color: palette.danger),
              ),
          ],
        ),
      ),
    );
  }
}

/// At the bottom of the margin (and the sheet) while there are drafts:
/// "Send 3 comments to June", one button for each agent they're to.
class _SendBar extends StatelessWidget {
  const _SendBar({required this.controller});

  final CommentsController controller;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final drafts = controller.drafts;
    if (drafts.isEmpty) return const SizedBox.shrink();
    return Container(
      padding: const EdgeInsets.all(Space.m),
      decoration: BoxDecoration(
        color: palette.canvas,
        border: Border(top: BorderSide(color: palette.hairline)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final MapEntry(key: agentId, value: count) in drafts.entries)
            Padding(
              padding: const EdgeInsets.only(bottom: Space.xs),
              child: Button(
                label:
                    'Send $count ${count == 1 ? 'comment' : 'comments'} to '
                    '${_agentName(controller.backend, agentId)}',
                primary: true,
                onPressed: () => controller.send(agentId),
              ),
            ),
        ],
      ),
    );
  }
}

String _agentName(Backend backend, String agentId) {
  for (final agent in backend.workspace.workspace.agents) {
    if (agent.id == agentId) return agent.name;
  }
  return agentId;
}

Agent? _agent(Backend backend, String agentId) {
  for (final agent in backend.workspace.workspace.agents) {
    if (agent.id == agentId) return agent;
  }
  return null;
}

/// A thread: what it's about, its comments, and what the person can do
/// with it. Picked, it stands out and offers Reply and Resolve.
class _ThreadCard extends StatelessWidget {
  const _ThreadCard({required this.controller, required this.thread});

  final CommentsController controller;
  final Thread thread;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final focused = controller.focused == thread.id;
    final resolved = thread.status == ThreadStatus.THREAD_STATUS_RESOLVED;
    final located = controller.isPlaced(thread.id);
    final region = controller.regions[thread.id];
    final moment = controller.moments[thread.id];
    final backend = controller.backend;
    final agent = _agent(backend, thread.agentId);
    final caption = TextStyles.caption.copyWith(color: palette.textMuted);
    final faint = TextStyles.caption.copyWith(color: palette.textFaint);
    final anchor = anchorIn(thread, controller.kind);
    final quote = located
        ? (anchor?.exact ?? thread.originalExact)
        : thread.originalExact;
    final suggestion = thread.hasSuggestion() ? thread.suggestion : null;
    final decision = suggestion?.decision;
    final undecided =
        decision == SuggestionDecision.SUGGESTION_DECISION_UNSPECIFIED;
    final lines = controller.kind == 'text' && thread.source.firstLine > 0
        ? thread.source.firstLine == thread.source.lastLine
              ? 'Line ${thread.source.firstLine}'
              : 'Lines ${thread.source.firstLine}–${thread.source.lastLine}'
        : null;
    final now = DateTime.now();
    return Opacity(
      opacity: resolved ? 0.6 : 1,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: () => controller.focus(focused ? null : thread.id),
        child: AnimatedContainer(
          duration: Motion.fast,
          padding: const EdgeInsets.all(Space.m),
          decoration: BoxDecoration(
            color: palette.canvas,
            borderRadius: BorderRadius.circular(Radii.panel),
            border: Border.all(
              color: focused ? palette.textMuted : palette.hairline,
            ),
            boxShadow: focused ? floatingShadow : null,
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Unselectable(
                child: Row(
                  children: [
                    if (controller.pins[thread.id] case final number?)
                      PinBadge(
                        number: number,
                        color: agentColor(agent?.color ?? ''),
                      )
                    else
                      Dot(color: agentColor(agent?.color ?? ''), size: 6),
                    const SizedBox(width: Space.xs + 2),
                    Expanded(
                      child: Text(
                        'To ${agent?.name ?? thread.agentId}',
                        style: faint,
                      ),
                    ),
                    if (decision ==
                        SuggestionDecision.SUGGESTION_DECISION_ACCEPTED)
                      Text('Accepted', style: faint)
                    else if (decision ==
                        SuggestionDecision.SUGGESTION_DECISION_REJECTED)
                      Text('Rejected', style: faint)
                    else if (resolved)
                      Text('Resolved', style: faint),
                  ],
                ),
              ),
              const SizedBox(height: Space.xs),
              if (moment != null) ...[
                if (thread.anchor == AnchorState.ANCHOR_STATE_OUTDATED) ...[
                  Text(
                    'Made on an earlier version of this video.',
                    style: faint,
                  ),
                  const SizedBox(height: Space.xs),
                ],
                _MomentLabel(moment: moment),
              ] else if (region != null) ...[
                if (thread.anchor == AnchorState.ANCHOR_STATE_OUTDATED) ...[
                  Text(
                    'Made on an earlier version of this image.',
                    style: faint,
                  ),
                  const SizedBox(height: Space.xs),
                ],
                RegionThumbnail(image: controller.image, region: region),
              ] else if (!located)
                Text(
                  thread.anchor == AnchorState.ANCHOR_STATE_DETACHED
                      ? 'The text it was on is gone.'
                      : "The text it was on isn't in this version.",
                  style: TextStyles.caption.copyWith(color: palette.danger),
                )
              else if (thread.anchor == AnchorState.ANCHOR_STATE_MOVED)
                Text(
                  suggestion != null && undecided
                      ? 'The text changed after this was suggested.'
                      : 'The text changed since this comment.',
                  style: faint,
                ),
              if (region != null || moment != null)
                const SizedBox.shrink()
              else if (suggestion != null)
                _Suggestion(
                  suggestion: suggestion,
                  lines: lines,
                  before: thread.originalExact.isNotEmpty
                      ? thread.originalExact
                      : quote,
                )
              else if (quote.isNotEmpty)
                Text(
                  [?lines, '“${quote.trim()}”'].join(' · '),
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: caption.copyWith(fontStyle: FontStyle.italic),
                ),
              // A suggestion's own comment may be empty: it only carries
              // the draft.
              for (final comment in thread.comments)
                if (comment.body.isNotEmpty ||
                    controller.editing == comment.id) ...[
                  const SizedBox(height: Space.s),
                  if (controller.editing == comment.id &&
                      !isPhoneLayout(context))
                    _Composer(
                      controller: controller,
                      initial: comment.body,
                      agentName: agent?.name ?? '',
                      canSendNow: false,
                    )
                  else
                    _Comment(
                      controller: controller,
                      thread: thread,
                      comment: comment,
                      now: now,
                    ),
                ],
              if (suggestion != null && undecided) ...[
                const SizedBox(height: Space.s),
                _Decide(controller: controller, thread: thread),
              ],
              if (controller.replyingTo == thread.id &&
                  !isPhoneLayout(context)) ...[
                const SizedBox(height: Space.s),
                _Composer(
                  controller: controller,
                  agentName: agent?.name ?? '',
                  placeholder: 'Reply',
                ),
              ] else if (focused) ...[
                const SizedBox(height: Space.s),
                Unselectable(
                  child: Wrap(
                    spacing: Space.xs,
                    runSpacing: Space.xs,
                    children: [
                      Button(
                        label: 'Reply',
                        onPressed: () => controller.startReply(thread.id),
                      ),
                      Button(
                        label: resolved ? 'Reopen' : 'Resolve',
                        onPressed: () =>
                            controller.resolve(thread, resolved: !resolved),
                      ),
                      if (!located)
                        ValueListenableBuilder(
                          valueListenable: controller.selection,
                          builder: (context, passage, _) => Button(
                            label: 'Move to selected text',
                            onPressed: passage == null || passage.isEmpty
                                ? null
                                : () => controller.reattach(thread),
                          ),
                        ),
                      _Readdress(controller: controller, thread: thread),
                    ],
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// A suggested change: the text as it was, struck through, and what
/// replaces it.
class _Suggestion extends StatelessWidget {
  const _Suggestion({
    required this.suggestion,
    required this.before,
    this.lines,
  });

  final Suggestion suggestion;
  final String before;
  final String? lines;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final caption = TextStyles.caption.copyWith(color: palette.textMuted);
    final lines = this.lines;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text.rich(
          TextSpan(
            children: [
              if (lines != null) TextSpan(text: '$lines · '),
              TextSpan(
                text: before.trim(),
                style: const TextStyle(decoration: TextDecoration.lineThrough),
              ),
            ],
          ),
          maxLines: 3,
          overflow: TextOverflow.ellipsis,
          style: caption,
        ),
        const SizedBox(height: Space.xs),
        if (suggestion.replacement.isEmpty)
          Text(
            'Delete this text',
            style: caption.copyWith(fontStyle: FontStyle.italic),
          )
        else
          Text(
            suggestion.replacement,
            style: TextStyles.body.copyWith(color: palette.text),
          ),
      ],
    );
  }
}

/// A suggestion not decided yet: the agent's to accept or reject, or, once
/// a comment is typed, to answer with it (Submit, or Return), the
/// suggestion left open; the person's own to apply or take back.
/// Accepting needs the text as it was.
class _Decide extends StatefulWidget {
  const _Decide({required this.controller, required this.thread});

  final CommentsController controller;
  final Thread thread;

  @override
  State<_Decide> createState() => _DecideState();
}

class _DecideState extends State<_Decide> {
  final _comment = TextEditingController();

  @override
  void dispose() {
    _comment.dispose();
    super.dispose();
  }

  void _decide({required bool accept}) => widget.controller.decide(
    widget.thread,
    accept: accept,
    comment: _comment.text.trim(),
  );

  /// The comment into the thread, as a reply: nothing accepted or
  /// rejected. Sent, the field empties for the next.
  Future<void> _reply() async {
    final sent = await widget.controller.decide(
      widget.thread,
      accept: false,
      comment: _comment.text.trim(),
      reply: true,
    );
    if (sent && mounted) _comment.clear();
  }

  @override
  Widget build(BuildContext context) {
    final thread = widget.thread;
    final controller = widget.controller;
    final own = thread.suggestion.authorAgentId.isEmpty;
    final canAccept =
        thread.anchor == AnchorState.ANCHOR_STATE_ANCHORED &&
        controller.located.containsKey(thread.id);
    final agent = own
        ? null
        : controller.backend.workspace.agent(thread.suggestion.authorAgentId);
    final accept = Button(
      label: own ? 'Apply' : 'Accept',
      primary: true,
      onPressed: canAccept ? () => _decide(accept: true) : null,
    );
    final reject = Button(
      label: own ? 'Discard' : 'Reject',
      onPressed: () => _decide(accept: false),
    );
    final name = agent?.name ?? 'The agent';
    return Unselectable(
      child: own
          ? CardRow(quiet: [reject], primary: accept)
          // The field, open from the start, above the row; words typed:
          // one Submit, saying what it does.
          : CardAnswerBlock(
              field: (focus) => DecisionComment(
                controller: _comment,
                focusNode: focus,
                agent: name,
                onSubmit: () => unawaited(_reply()),
              ),
              row: CommentTyped(
                controller: _comment,
                builder: (context, typed) => typed
                    ? CardRow(
                        line: CardLine(insteadLine('accepting')),
                        primary: SubmitButton(
                          onPressed: () => unawaited(_reply()),
                        ),
                      )
                    : CardRow(quiet: [reject], primary: accept),
              ),
            ),
    );
  }
}

/// Who the thread is to, to change it: a menu of the agents.
class _Readdress extends StatelessWidget {
  const _Readdress({required this.controller, required this.thread});

  final CommentsController controller;
  final Thread thread;

  @override
  Widget build(BuildContext context) {
    final agents = controller.backend.workspace.workspace.agents;
    if (agents.length < 2) return const SizedBox.shrink();
    return Select<String>(
      value: thread.agentId,
      options: [
        for (final agent in agents) SelectOption(agent.id, 'To ${agent.name}'),
      ],
      onChanged: (agentId) {
        if (agentId != thread.agentId) controller.readdress(thread, agentId);
      },
    );
  }
}

/// One comment: who wrote it and when, a draft's mark, and what it says.
/// The person's own can be edited and deleted.
class _Comment extends StatelessWidget {
  const _Comment({
    required this.controller,
    required this.thread,
    required this.comment,
    required this.now,
  });

  final CommentsController controller;
  final Thread thread;
  final ThreadComment comment;
  final DateTime now;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final mine = comment.authorAgentId.isEmpty;
    final author = mine
        ? 'You'
        : _agentName(controller.backend, comment.authorAgentId);
    final type = TypeScale.of(context);
    final faint = type.caption.copyWith(color: palette.textFaint);
    final focused = controller.focused == thread.id;
    // At the largest text on a phone, when and Edit and Delete go under
    // who wrote it, each with the width.
    final stacked = isPhoneLayout(context) && largeText(context);
    final actions = mine && focused
        ? [
            _TextAction(
              label: 'Edit',
              onTap: () => controller.startEdit(comment.id, thread.id),
            ),
            _TextAction(
              label: 'Delete',
              onTap: () => controller.delete(comment),
            ),
          ]
        : const <Widget>[];
    final age = Text(
      comment.draft ? 'Draft' : formatAge(comment.createdMs.toInt(), now),
      style: faint,
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Unselectable(
          child: Row(
            children: [
              if (mine) ...[
                const PersonAvatar(size: 16),
                const SizedBox(width: Space.xs + 2),
              ],
              Flexible(
                child: Text(
                  author,
                  style: type.caption.copyWith(
                    color: palette.text,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ),
              if (!stacked) ...[
                const SizedBox(width: Space.xs + 2),
                Expanded(child: age),
                for (final (i, action) in actions.indexed) ...[
                  if (i > 0) const SizedBox(width: Space.s),
                  action,
                ],
              ],
            ],
          ),
        ),
        if (stacked) ...[
          Unselectable(child: age),
          if (actions.isNotEmpty)
            Unselectable(
              child: Wrap(spacing: Space.s, children: actions),
            ),
        ],
        const SizedBox(height: 2),
        Text(comment.body, style: type.body.copyWith(color: palette.text)),
      ],
    );
  }
}

class _TextAction extends StatelessWidget {
  const _TextAction({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = isPhoneLayout(context);
    final text = Pressable(
      onTap: onTap,
      builder: (context, hovered) {
        final label = Text(
          this.label,
          style: TypeScale.of(context).caption
              .copyWith(color: hovered ? palette.text : palette.textMuted),
        );
        // A finger's size on a phone.
        return phone
            ? ConstrainedBox(
                constraints: const BoxConstraints(
                  minWidth: Touch.target,
                  minHeight: Touch.target,
                ),
                child: Center(widthFactor: 1, child: label),
              )
            : label;
      },
    );
    return Semantics(
      container: true,
      button: true,
      label: label,
      onTap: onTap,
      excludeSemantics: true,
      child: text,
    );
  }
}

/// The popover a new comment is written in, at what it's on.
class _ComposerCard extends StatelessWidget {
  const _ComposerCard({required this.controller, required this.mark});

  final CommentsController controller;
  final Mark mark;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final suggesting = controller.suggesting;
    final mark = this.mark;
    return Container(
      padding: const EdgeInsets.all(Space.m),
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
        boxShadow: floatingShadow,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          switch (mark) {
            Region(:final rect) => RegionThumbnail(
              image: controller.image,
              region: rect,
            ),
            Moment() => _MomentLabel(moment: mark),
            Passage(:final exact) => Text(
              suggesting ? exact.trim() : '“${exact.trim()}”',
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyles.caption.copyWith(
                color: palette.textMuted,
                fontStyle: suggesting ? null : FontStyle.italic,
                decoration: suggesting ? TextDecoration.lineThrough : null,
              ),
            ),
          },
          const SizedBox(height: Space.s),
          _Composer(
            // A new one when it turns from a comment to a suggestion.
            key: ValueKey(suggesting),
            controller: controller,
            agentName: _agentName(controller.backend, controller.defaultAgent),
            replacement: suggesting && mark is Passage ? mark.exact : null,
          ),
        ],
      ),
    );
  }
}

/// Where in the video a comment is: "0:31–0:45", and whether it's on a
/// part of the frame.
class _MomentLabel extends StatelessWidget {
  const _MomentLabel({required this.moment});

  final Moment moment;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Text(
      moment.region == null
          ? moment.label
          : '${moment.label} · part of the frame',
      style: TextStyles.caption.copyWith(
        color: palette.textMuted,
        fontFeatures: const [FontFeature.tabularFigures()],
      ),
    );
  }
}

/// A region of the image as a small picture: what a comment is on. Its
/// size follows the region's shape, at most 72 high and as wide as there's
/// room for.
class RegionThumbnail extends StatelessWidget {
  const RegionThumbnail({super.key, required this.image, required this.region});

  final ui.Image? image;
  final Rect region;

  @override
  Widget build(BuildContext context) {
    final image = this.image;
    if (image == null) return const SizedBox.shrink();
    final palette = Palette.of(context);
    final whole =
        Offset.zero & Size(image.width.toDouble(), image.height.toDouble());
    final point = region.width == 0 && region.height == 0;
    final spot = Offset(region.left * image.width, region.top * image.height);
    // A spot shows with what's around it, a fifth of the image's shorter
    // side across, and a dot on it.
    final side = (image.width < image.height ? image.width : image.height) / 5;
    final source = point
        ? Rect.fromCenter(
            center: spot,
            width: side,
            height: side,
          ).intersect(whole)
        : Rect.fromLTWH(
            region.left * image.width,
            region.top * image.height,
            region.width * image.width,
            region.height * image.height,
          ).intersect(whole);
    if (source.isEmpty) return const SizedBox.shrink();
    return LayoutBuilder(
      builder: (context, constraints) {
        final scale = [
          72 / source.height,
          constraints.maxWidth / source.width,
          1.0,
        ].reduce((a, b) => a < b ? a : b);
        return Align(
          alignment: Alignment.centerLeft,
          child: Container(
            width: source.width * scale + 2,
            height: source.height * scale + 2,
            decoration: BoxDecoration(
              border: Border.all(color: palette.hairline),
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            clipBehavior: Clip.antiAlias,
            child: CustomPaint(
              painter: _Crop(
                image,
                source,
                spot: point ? spot : null,
                color: palette.signal,
              ),
            ),
          ),
        );
      },
    );
  }
}

class _Crop extends CustomPainter {
  _Crop(this.image, this.source, {this.spot, required this.color});

  final ui.Image image;
  final Rect source;

  /// A spot in the image to mark with a dot.
  final Offset? spot;
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawImageRect(
      image,
      source,
      Offset.zero & size,
      Paint()..filterQuality = FilterQuality.medium,
    );
    final spot = this.spot;
    if (spot == null) return;
    final at = Offset(
      (spot.dx - source.left) / source.width * size.width,
      (spot.dy - source.top) / source.height * size.height,
    );
    canvas
      ..drawCircle(at, 4.5, Paint()..color = const Color(0xFFFFFFFF))
      ..drawCircle(at, 3, Paint()..color = color);
  }

  @override
  bool shouldRepaint(_Crop old) =>
      old.image != image ||
      old.source != source ||
      old.spot != spot ||
      old.color != color;
}

/// A comment's pin on an image or a video's frame, painted at [at]: a
/// circle of its agent's [color], 18px (22px while [strong], picked)
/// ringed in [ring] (the canvas), with its [number] in white; without one,
/// a small dot (the person's own mark).
void paintPin(
  Canvas canvas,
  Offset at, {
  required Color color,
  required Color ring,
  int? number,
  bool strong = false,
}) {
  final radius = number == null ? 4.0 : (strong ? 11.0 : 9.0);
  canvas
    ..drawCircle(at, radius + 1.5, Paint()..color = ring)
    ..drawCircle(at, radius, Paint()..color = color);
  if (number == null) return;
  final label = TextPainter(
    text: TextSpan(
      text: '$number',
      style: const TextStyle(
        fontFamily: TextStyles.sans,
        fontSize: 11,
        height: 1,
        fontWeight: FontWeight.w600,
        color: Color(0xFFFFFFFF),
      ),
    ),
    textDirection: TextDirection.ltr,
  )..layout();
  label.paint(canvas, at - Offset(label.width / 2, label.height / 2));
  label.dispose();
}

/// A pin's number in a card's header, matching the pin on the image: a
/// 16px circle of the agent's color with the number in white.
class PinBadge extends StatelessWidget {
  const PinBadge({super.key, required this.number, required this.color});

  final int number;
  final Color color;

  @override
  Widget build(BuildContext context) => Container(
    width: 16,
    height: 16,
    alignment: Alignment.center,
    decoration: BoxDecoration(color: color, shape: BoxShape.circle),
    child: Text(
      '$number',
      style: const TextStyle(
        fontFamily: TextStyles.sans,
        fontSize: 10,
        height: 1,
        fontWeight: FontWeight.w600,
        color: Color(0xFFFFFFFF),
      ),
    ),
  );
}

/// Over an image or a video's frame: a floating Comment button, shown
/// while the pointer is over it (always on a phone), that starts picking
/// where the comment goes; while picking, what to do and Cancel.
class CommentPick extends StatelessWidget {
  const CommentPick({
    super.key,
    required this.picking,
    required this.visible,
    required this.hint,
    required this.onToggle,
  });

  final bool picking;
  final bool visible;
  final String hint;
  final VoidCallback onToggle;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final shown = picking || visible;
    return IgnorePointer(
      ignoring: !shown,
      child: AnimatedOpacity(
        opacity: shown ? 1 : 0,
        duration: shown ? Motion.fast : Motion.fast * 0.75,
        curve: Curves.easeOut,
        child: Unselectable(
          child: _Floating(
            child: picking
                ? Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Flexible(
                        child: Padding(
                          padding: const EdgeInsets.symmetric(
                            horizontal: Space.s,
                          ),
                          child: Text(
                            hint,
                            style: TextStyles.caption.copyWith(
                              color: palette.textMuted,
                            ),
                          ),
                        ),
                      ),
                      Button(label: 'Cancel', onPressed: onToggle),
                    ],
                  )
                : Button(label: 'Comment', primary: true, onPressed: onToggle),
          ),
        ),
      ),
    );
  }
}

/// Where a comment is written: Return sends it to the agent at once (an
/// edit is saved), Shift-Return makes a new line, Esc gives up. Suggesting
/// a change, the new text comes first, and the comment is optional.
class _Composer extends StatefulWidget {
  const _Composer({
    super.key,
    required this.controller,
    required this.agentName,
    this.initial = '',
    this.placeholder = 'Comment',
    this.canSendNow = true,
    this.replacement,
  });

  final CommentsController controller;
  final String agentName;
  final String initial;
  final String placeholder;

  /// False while editing a comment: an edit is only saved.
  final bool canSendNow;

  /// Suggesting a change: the text it starts from.
  final String? replacement;

  @override
  State<_Composer> createState() => _ComposerState();
}

class _ComposerState extends State<_Composer> {
  late final _text = TextEditingController(text: widget.initial);
  late final _replacement = widget.replacement == null
      ? null
      : TextEditingController(text: widget.replacement);
  var _busy = false;

  @override
  void dispose() {
    _text.dispose();
    _replacement?.dispose();
    super.dispose();
  }

  /// A suggestion needs a change; a comment needs words.
  bool get _ready {
    final replacement = _replacement;
    if (replacement == null) return _text.text.trim().isNotEmpty;
    return replacement.text != widget.replacement;
  }

  Future<void> _submit() async {
    if (_busy || !_ready) return;
    setState(() => _busy = true);
    await widget.controller.submit(
      _text.text,
      sendNow: widget.canSendNow,
      replacement: _replacement?.text,
    );
    if (mounted) setState(() => _busy = false);
  }

  /// Return sends; Shift-Return is left to the field, a new line, and so
  /// is a Return that ends an input method's composing.
  KeyEventResult _key(FocusNode node, KeyEvent event) {
    if (event is KeyUpEvent) return KeyEventResult.ignored;
    final key = event.logicalKey;
    if (key == LogicalKeyboardKey.escape) {
      widget.controller.cancel();
      return KeyEventResult.handled;
    }
    if (key != LogicalKeyboardKey.enter &&
        key != LogicalKeyboardKey.numpadEnter) {
      return KeyEventResult.ignored;
    }
    if (HardwareKeyboard.instance.isShiftPressed ||
        _text.value.composing.isValid ||
        (_replacement?.value.composing.isValid ?? false)) {
      return KeyEventResult.ignored;
    }
    if (event is KeyDownEvent) unawaited(_submit());
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final replacement = _replacement;
    final replying = widget.placeholder == 'Reply';
    return Focus(
      canRequestFocus: false,
      skipTraversal: true,
      onKeyEvent: _key,
      // A scope of its own, so its field takes the keys as it opens even
      // when the viewer under it has them (the click that opened it gave
      // them to a video's player): a field's autofocus waits for a scope
      // with nothing focused.
      child: FocusScope(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (replacement != null) ...[
              TextField(
                controller: replacement,
                placeholder: 'New text',
                minLines: 2,
                maxLines: 8,
                autofocus: true,
              ),
              const SizedBox(height: Space.s),
            ],
            TextField(
              controller: _text,
              placeholder: replacement != null
                  ? 'Comment (optional)'
                  : widget.placeholder,
              minLines: replacement != null ? 1 : 2,
              maxLines: 8,
              autofocus: replacement == null,
            ),
            const SizedBox(height: Space.s),
            Unselectable(
              child: Wrap(
                alignment: WrapAlignment.end,
                spacing: Space.xs,
                runSpacing: Space.xs,
                children: [
                  Button(label: 'Cancel', onPressed: widget.controller.cancel),
                  ListenableBuilder(
                    listenable: Listenable.merge([_text, ?replacement]),
                    builder: (context, _) => Button(
                      label: replacement != null
                          ? 'Suggest'
                          : widget.initial.isNotEmpty
                          ? 'Save'
                          : replying
                          ? 'Reply'
                          : 'Comment',
                      primary: true,
                      onPressed: _busy || !_ready ? null : _submit,
                    ),
                  ),
                ],
              ),
            ),
            if (widget.canSendNow && widget.agentName.isNotEmpty) ...[
              const SizedBox(height: Space.xs),
              Text(
                '↩ sends to ${widget.agentName} · ⇧↩ starts a new line',
                textAlign: TextAlign.right,
                style: TextStyles.caption.copyWith(color: palette.textFaint),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// The narrow pane's comments, from the bottom: every thread in the order
/// of its passage, then the Send bar. A tap above it closes it.
class _Sheet extends StatelessWidget {
  const _Sheet({required this.controller});

  final CommentsController controller;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ListenableBuilder(
      listenable: controller,
      builder: (context, _) {
        if (!controller.sheetOpen) return const SizedBox.shrink();
        final threads = controller.inOrder(controller.shown);
        return LayoutBuilder(
          builder: (context, constraints) => Stack(
            children: [
              Positioned.fill(
                child: GestureDetector(
                  onTap: () => controller.openSheet(false),
                  child: const ColoredBox(color: Color(0x4D000000)),
                ),
              ),
              Align(
                alignment: Alignment.bottomCenter,
                child: Container(
                  constraints: BoxConstraints(
                    maxHeight: constraints.maxHeight * 0.85,
                  ),
                  decoration: BoxDecoration(
                    color: palette.sidebar,
                    borderRadius: const BorderRadius.vertical(
                      top: Radius.circular(Radii.floating),
                    ),
                    border: Border.all(color: palette.hairline),
                    boxShadow: floatingShadow,
                  ),
                  clipBehavior: Clip.antiAlias,
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      _MarginHeader(controller: controller),
                      Flexible(
                        child: ListView(
                          shrinkWrap: true,
                          padding: const EdgeInsets.all(Space.m),
                          children: [
                            if (threads.isEmpty)
                              Padding(
                                padding: const EdgeInsets.all(Space.m),
                                child: Text(
                                  switch (controller.kind) {
                                    'image' =>
                                      'Turn on Comment, then drag across '
                                          'the part of the image you want '
                                          'to comment on.',
                                    'video' =>
                                      'Turn on Comment, then '
                                          '${defaultTargetPlatform == TargetPlatform.iOS ? 'tap' : 'click'} '
                                          'or drag along the timeline to '
                                          'comment on a moment or a stretch '
                                          'of time. Drag across the frame to '
                                          'point at part of it.',
                                    _ => 'Select text to comment on it.',
                                  },
                                  textAlign: TextAlign.center,
                                  style: TextStyles.body.copyWith(
                                    color: palette.textMuted,
                                  ),
                                ),
                              ),
                            for (final thread in threads) ...[
                              _ThreadCard(
                                controller: controller,
                                thread: thread,
                              ),
                              const SizedBox(height: Space.s),
                            ],
                          ],
                        ),
                      ),
                      _SendBar(controller: controller),
                    ],
                  ),
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

/// A phone's header button for the comments: the glyph and how many,
/// opening them in a sheet.
class _PhoneCommentsButton extends StatelessWidget {
  const _PhoneCommentsButton({required this.controller, required this.count});

  final CommentsController controller;
  final int count;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      container: true,
      button: true,
      label: switch (count) {
        0 => 'Comments',
        1 => '1 comment',
        _ => '$count comments',
      },
      onTap: () => controller.openSheet(true),
      excludeSemantics: true,
      child: Pressable(
        onTap: () => controller.openSheet(true),
        builder: (context, pressed) => ConstrainedBox(
          constraints: const BoxConstraints(
            minWidth: Touch.target,
            minHeight: Touch.target,
          ),
          child: Center(
            widthFactor: 1,
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: Space.xs),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  ExtensionGlyph(
                    icon: 'message-square',
                    name: 'Comments',
                    color: pressed ? palette.textMuted : palette.text,
                    size: 20,
                  ),
                  if (count > 0) ...[
                    const SizedBox(width: Space.xs),
                    Text(
                      '$count',
                      style: TypeScale.of(context).secondary.copyWith(
                        color: palette.text,
                        fontFeatures: const [FontFeature.tabularFigures()],
                      ),
                    ),
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

/// On a phone, the comments in sheets rather than over the file (research
/// mobile.md §4): Comment or Suggest from the text's own menu writes in a
/// sheet above the keyboard; a marker opens its thread in a sheet; the
/// header's count lists them all, with Send for those kept to send. Builds
/// nothing itself: it presents the sheets as the controller asks.
class _PhoneComments extends StatefulWidget {
  const _PhoneComments({required this.controller});

  final CommentsController controller;

  @override
  State<_PhoneComments> createState() => _PhoneCommentsState();
}

class _PhoneCommentsState extends State<_PhoneComments> {
  var _writing = false;
  var _listing = false;
  String? _reading;

  CommentsController get _comments => widget.controller;

  @override
  void initState() {
    super.initState();
    _comments.addListener(_changed);
  }

  @override
  void didUpdateWidget(_PhoneComments old) {
    super.didUpdateWidget(old);
    if (old.controller != widget.controller) {
      old.controller.removeListener(_changed);
      widget.controller.addListener(_changed);
    }
  }

  @override
  void dispose() {
    _comments.removeListener(_changed);
    super.dispose();
  }

  var _due = false;

  /// After the frame: the controller tells while it's changing.
  void _changed() {
    if (_due) return;
    _due = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _due = false;
      if (mounted) _sync();
    });
    WidgetsBinding.instance.ensureVisualUpdate();
  }

  void _sync() {
    final comments = _comments;
    final writing =
        comments.composing != null ||
        comments.replyingTo != null ||
        comments.editing != null;
    if (writing && !_writing) {
      unawaited(_write());
    } else if (comments.sheetOpen && !_listing) {
      unawaited(_list());
    } else if (!comments.sheetOpen &&
        !_listing &&
        !_writing &&
        _reading == null &&
        comments.focused != null) {
      unawaited(_read(comments.focused!));
    }
  }

  Thread? _thread(String id) =>
      _comments.all.where((thread) => thread.id == id).firstOrNull;

  /// What a thread is on, for the top of its reply.
  Widget _quoteOf(Thread thread) {
    final region = _comments.regions[thread.id];
    if (region != null) {
      return RegionThumbnail(image: _comments.image, region: region);
    }
    final moment = _comments.moments[thread.id];
    if (moment != null) return _MomentLabel(moment: moment);
    final anchor = anchorIn(thread, _comments.kind);
    return QuotedPassage(anchor?.exact ?? thread.originalExact);
  }

  Future<void> _write() async {
    final comments = _comments;
    _writing = true;
    final backend = comments.backend;
    final editing = comments.editing;
    final replyingTo = comments.replyingTo;
    final mark = comments.composing;
    late final CommentEditing kind;
    late final Widget quote;
    var recipient = '';
    var initial = '';
    String? replacing;
    if (editing != null) {
      final thread = comments.all
          .where((t) => t.comments.any((c) => c.id == editing))
          .firstOrNull;
      kind = CommentEditing.edit;
      quote = thread == null ? const SizedBox.shrink() : _quoteOf(thread);
      initial = thread?.comments.firstWhere((c) => c.id == editing).body ?? '';
    } else if (replyingTo != null) {
      final thread = _thread(replyingTo);
      kind = CommentEditing.reply;
      quote = thread == null ? const SizedBox.shrink() : _quoteOf(thread);
      recipient = _agentName(backend, thread?.agentId ?? '');
    } else {
      final suggest = comments.suggesting && mark is Passage;
      kind = suggest ? CommentEditing.suggest : CommentEditing.comment;
      recipient = _agentName(backend, comments.defaultAgent);
      quote = switch (mark) {
        Region(:final rect) => RegionThumbnail(
          image: comments.image,
          region: rect,
        ),
        Moment() => _MomentLabel(moment: mark),
        Passage(:final exact) => QuotedPassage(exact, struck: suggest),
        null => const SizedBox.shrink(),
      };
      if (suggest) replacing = mark.exact;
    }
    comments.error = null;
    await showCommentEditor(
      context,
      editing: kind,
      quote: quote,
      recipient: recipient,
      initial: initial,
      replacing: replacing,
      error: comments.failure,
      submit: (body, replacement) => comments.submit(
        body,
        sendNow: kind != CommentEditing.edit,
        replacement: replacement,
      ),
    );
    _writing = false;
    if (!mounted) return;
    // Closed without sending: nothing's being written any more.
    if (comments.composing != null ||
        comments.replyingTo != null ||
        comments.editing != null) {
      comments.cancel();
    }
  }

  Future<void> _list() async {
    _listing = true;
    await presentSheet<void>(
      context,
      title: 'Comments',
      tall: _comments.shown.length > 1,
      padded: false,
      builder: (context, close) => _PhoneThreadList(controller: _comments),
    );
    _listing = false;
    if (!mounted) return;
    if (_comments.sheetOpen) _comments.openSheet(false);
    _comments.letGo();
  }

  Future<void> _read(String threadId) async {
    _reading = threadId;
    await presentSheet<void>(
      context,
      padded: false,
      builder: (context, close) => ListenableBuilder(
        listenable: _comments,
        builder: (context, _) {
          final thread = _thread(threadId);
          if (thread == null) {
            // Deleted meanwhile.
            WidgetsBinding.instance.addPostFrameCallback((_) => close());
            return const SizedBox.shrink();
          }
          return SingleChildScrollView(
            padding: const EdgeInsets.fromLTRB(
              Touch.gutter,
              0,
              Touch.gutter,
              Touch.gutter,
            ),
            child: _ThreadCard(controller: _comments, thread: thread),
          );
        },
      ),
    );
    _reading = null;
    if (!mounted) return;
    if (_comments.focused == threadId) _comments.letGo();
  }

  @override
  Widget build(BuildContext context) => const SizedBox.shrink();
}

/// A phone's list of the comments on a file, in a sheet: each thread in
/// reading order (a tap opens what can be done with it), whether resolved
/// ones show, and Send for comments kept to send.
class _PhoneThreadList extends StatelessWidget {
  const _PhoneThreadList({required this.controller});

  final CommentsController controller;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return ListenableBuilder(
      listenable: controller,
      builder: (context, _) {
        final threads = controller.inOrder(controller.shown);
        final resolved = controller.resolvedCount;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Flexible(
              child: ListView(
                shrinkWrap: true,
                padding: const EdgeInsets.fromLTRB(
                  Touch.gutter,
                  0,
                  Touch.gutter,
                  Touch.gutter,
                ),
                children: [
                  if (threads.isEmpty)
                    Padding(
                      padding: const EdgeInsets.symmetric(vertical: Space.l),
                      child: Text(
                        switch (controller.kind) {
                          'image' =>
                            'Tap Comment, then tap or drag across the part '
                                'of the image you want to comment on.',
                          'video' =>
                            'Tap Comment to comment on the moment playing, '
                                'or drag across the frame to point at part '
                                'of it.',
                          _ =>
                            'Touch and hold text to select it, then tap '
                                'Comment.',
                        },
                        textAlign: TextAlign.center,
                        style: type.body.copyWith(color: palette.textMuted),
                      ),
                    ),
                  for (final thread in threads) ...[
                    _ThreadCard(controller: controller, thread: thread),
                    const SizedBox(height: Space.s),
                  ],
                  if (resolved > 0)
                    Center(
                      child: Button(
                        label: controller.showResolved
                            ? 'Hide resolved'
                            : 'Show $resolved resolved',
                        onPressed: controller.toggleResolved,
                      ),
                    ),
                ],
              ),
            ),
            _SendBar(controller: controller),
          ],
        );
      },
    );
  }
}

/// A phone's bar under an image or a video, in the thumb's reach. At rest
/// it holds Comment, which turns on picking what to comment on. Picking,
/// [hint] says how, Cancel stops, and [action] is what needs no gesture:
/// the whole image, or a video's moment picked so far, written about at
/// once ([actionPrimary]).
class CommentModeBar extends StatelessWidget {
  const CommentModeBar({
    super.key,
    required this.marking,
    required this.onToggle,
    required this.hint,
    this.action,
    this.onAction,
    this.actionPrimary = false,
  });

  final bool marking;
  final VoidCallback onToggle;
  final String hint;

  /// While picking: a button beside Cancel, and what it does.
  final String? action;
  final VoidCallback? onAction;
  final bool actionPrimary;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final action = this.action;
    return Unselectable(
      child: Container(
        padding: const EdgeInsets.symmetric(
          horizontal: Touch.gutter,
          vertical: Space.xs,
        ),
        decoration: BoxDecoration(
          color: palette.sidebar,
          border: Border(top: BorderSide(color: palette.hairline)),
        ),
        child: marking
            ? Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Padding(
                    padding: const EdgeInsets.symmetric(vertical: Space.xs),
                    child: Semantics(
                      liveRegion: true,
                      child: Text(
                        hint,
                        style: type.secondary.copyWith(
                          color: palette.textMuted,
                        ),
                      ),
                    ),
                  ),
                  // Cancel at the leading end, the way forward at the
                  // trailing one, as iOS's bars have them; at the largest
                  // text, one over the other, the way forward on top.
                  OverflowBar(
                    alignment: MainAxisAlignment.spaceBetween,
                    spacing: Space.s,
                    overflowSpacing: Space.xs,
                    overflowAlignment: OverflowBarAlignment.end,
                    overflowDirection: VerticalDirection.up,
                    children: [
                      Button(label: 'Cancel', onPressed: onToggle),
                      if (action != null)
                        Button(
                          label: action,
                          primary: actionPrimary,
                          onPressed: onAction,
                        ),
                    ],
                  ),
                ],
              )
            : Align(
                alignment: AlignmentDirectional.centerEnd,
                child: Button(
                  label: 'Comment',
                  primary: true,
                  onPressed: onToggle,
                ),
              ),
      ),
    );
  }
}
