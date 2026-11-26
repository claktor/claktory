import 'dart:async';

import 'package:flutter/widgets.dart';

import '../chats.dart' show chatTitle;
import '../claktory_identity.dart';
import '../connection.dart';
import '../controls.dart';
import '../devices.dart' show thisDeviceKind;
import '../identity.dart';
import '../mobile/drawer.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../rail.dart';
import '../scope.dart';
import 'incoming_shares.dart';

/// A real shared conversation, not a parallel composer or a new destination.
class AndroidShareDestination {
  AndroidShareDestination.agent(this.claktory, Agent agent)
    : id = agent.id,
      name = agent.name,
      isChat = false;
  AndroidShareDestination.chat(this.claktory, Chat chat)
    : id = chat.id,
      name = chatTitle(chat),
      isChat = true;

  final Claktory claktory;
  final String id;
  final String name;
  final bool isChat;
  String get draftKey => isChat ? 'chat:$id' : id;
  bool get exists => isChat
      ? claktory.backend.workspace.workspace.chats.any((chat) => chat.id == id)
      : claktory.backend.workspace.agent(id) != null;
  bool get modelAvailable => isChat
      ? claktory.backend.workspace.workspace.chats
                .where((chat) => chat.id == id)
                .firstOrNull
                ?.modelAvailable ??
            false
      : claktory.backend.workspace.agent(id)?.modelAvailable ?? false;
  Future<void> select() => isChat
      ? claktory.backend.selectChat(id)
      : claktory.backend.selectAgent(id);
}

List<AndroidShareDestination> androidShareDestinations(Claktories roots) => [
  for (final root in roots.all)
    if (root.backend.workspace.received) ...[
      for (final agent in root.backend.workspace.workspace.agents)
        AndroidShareDestination.agent(root, agent),
      for (final chat in root.backend.workspace.workspace.chats)
        AndroidShareDestination.chat(root, chat),
    ],
];

/// Native errors deliberately have no failed filename/count. Never imply that
/// the first 20 files survived: native rejects ALL files when there are >20.
String? androidShareError(String? error) => switch (error) {
  null => null,
  'too_many' =>
    'You can share up to 20 files at once. Share fewer and try again.',
  'refused' => "Some files couldn't be opened.",
  'too_large' => "Files over 2 GB can't be added.",
  'no_space' => 'Not enough space on this $thisDeviceKind to add the files.',
  _ => "Some files couldn't be added. Try sharing them again.",
};

/// Mount INSIDE the window Overlay, above its phone/wide switch, and without a
/// per-Claktory key. The one queue survives window changes. Back remains owned
/// by AndroidWindowBackHost above that Overlay, never by this intake host.
class AndroidIncomingShareHost extends StatefulWidget {
  const AndroidIncomingShareHost({super.key, required this.child, this.shares});

  final Widget child;

  /// An injected queue is owned by its caller; the default native queue is
  /// owned here. Only one host/listener may be mounted in a window.
  final AndroidIncomingShares? shares;

  @override
  State<AndroidIncomingShareHost> createState() =>
      _AndroidIncomingShareHostState();
}

class _AndroidIncomingShareHostState extends State<AndroidIncomingShareHost> {
  late final _shares = widget.shares ?? AndroidIncomingShares();
  Claktories? _roots;
  bool _presenting = false;
  ValueNotifier<AndroidIncomingShare>? _failureNotice;
  Set<int>? _failureIds;

  @override
  void initState() {
    super.initState();
    _shares.addListener(_changed);
    unawaited(_shares.start());
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _roots = ClaktoriesScope.of(context);
    _changed();
  }

  @override
  void dispose() {
    _shares.removeListener(_changed);
    if (widget.shares == null) _shares.dispose();
    super.dispose();
  }

  void _changed() {
    if (!mounted || !_shares.ready || _roots == null) return;
    _mergeFailures();
    if (_presenting || _shares.pending.isEmpty) return;
    _presenting = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) unawaited(_present());
    });
    WidgetsBinding.instance.ensureVisualUpdate();
  }

  /// Repeated unreadable shares update the notice already on screen.
  /// Stop at real content: neither it nor an error behind it may skip its
  /// turn, and no accepted file or draft is discarded by this coalescing.
  void _mergeFailures() {
    final notice = _failureNotice;
    final ids = _failureIds;
    if (notice == null || ids == null) return;
    var latest = notice.value;
    for (final share in _shares.pending) {
      if (ids.contains(share.id)) continue;
      if (share.hasContent) break;
      ids.add(share.id);
      latest = share;
    }
    if (!identical(notice.value, latest)) notice.value = latest;
  }

  Future<void> _present() async {
    final roots = _roots!;
    if (_shares.pending.isEmpty) {
      _presenting = false;
      return;
    }
    final share = _shares.pending.first;
    final failures = share.hasContent ? null : <int>{share.id};
    final notice = share.hasContent ? null : ValueNotifier(share);
    _failureIds = failures;
    _failureNotice = notice;
    _mergeFailures();
    Future<String?>? delivery;
    PhoneDrawerHost.closeAll();
    AndroidSharePicker picker(
      AndroidIncomingShare current,
      VoidCallback close,
    ) => AndroidSharePicker(
      share: current,
      roots: roots,
      close: close,
      chosen: (destination) {
        // Do this synchronously BEFORE a Claktory switch can retire this
        // overlay. Its eventual close/cancel cannot delete accepted files.
        final claimed = _shares.claim(current.id);
        if (claimed != null) {
          delivery = _deliver(claimed, destination, roots);
        }
        close();
      },
    );
    try {
      await showModal(
        context,
        (context, close) => notice == null
            ? picker(share, close)
            : ValueListenableBuilder<AndroidIncomingShare>(
                valueListenable: notice,
                builder: (context, current, _) => picker(current, close),
              ),
      );
      // A later share, received while native cleanup is still finishing,
      // belongs to the next presentation, not the notice just dismissed.
      _failureNotice = null;
      _failureIds = null;
      final transferred = delivery;
      if (transferred == null) {
        for (final id in failures ?? {share.id}) {
          await _shares.discard(id);
        }
      } else {
        final warning = await transferred;
        if (mounted && warning != null) {
          // Rebind to the chosen window's scope before presenting its notice.
          await WidgetsBinding.instance.endOfFrame;
          if (mounted) {
            await showModal(
              context,
              (context, close) => Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Text(
                    warning,
                    style: TextStyles.body.copyWith(
                      color: Palette.of(context).text,
                    ),
                  ),
                  const SizedBox(height: Space.l),
                  Button(label: 'Close', onPressed: close),
                ],
              ),
            );
          }
        }
      }
    } finally {
      _failureNotice = null;
      _failureIds = null;
      notice?.dispose();
      _presenting = false;
      if (mounted) _changed();
    }
  }

  Future<String?> _deliver(
    AndroidIncomingShare share,
    AndroidShareDestination destination,
    Claktories roots,
  ) async {
    // Chosen already selected/validated the actual backend conversation. Draft
    // and file stores are the same ones its real MessageBox reads. Never Send.
    final claktory = destination.claktory;
    if (share.draftText.isNotEmpty) {
      claktory.drafts.compose(destination.draftKey, share.draftText);
    }
    var left = 0;
    try {
      if (share.files.isNotEmpty) {
        left = await claktory.backend.attachments.add(
          destination.draftKey,
          share.files,
        );
      }
    } catch (_) {
      left = share.files.length;
    }
    if (!mounted) return null;
    if (!roots.all.contains(claktory) || !destination.exists) {
      _shares.retryClaimed(share);
      return 'That conversation is gone. Choose another.';
    }
    claktory.history.clear();
    claktory.page.value = AppPage.agent;
    roots.select(claktory);
    PhoneClaktories.showConversation();
    return left == 0 ? null : androidShareError('copy_failed');
  }

  @override
  Widget build(BuildContext context) => widget.child;
}

/// Android presentation only: lists the existing agents/chats; it never owns a
/// draft, selects a model, creates a conversation, or sends a message.
class AndroidSharePicker extends StatefulWidget {
  const AndroidSharePicker({
    super.key,
    required this.share,
    required this.roots,
    required this.close,
    required this.chosen,
  });
  final AndroidIncomingShare share;
  final Claktories roots;
  final VoidCallback close;
  final ValueChanged<AndroidShareDestination> chosen;

  @override
  State<AndroidSharePicker> createState() => _AndroidSharePickerState();
}

class _AndroidSharePickerState extends State<AndroidSharePicker> {
  final _watched = <Listenable>[];
  bool _busy = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    widget.roots.addListener(_rootsChanged);
    _watch();
  }

  void _watch() {
    for (final watch in _watched) {
      watch.removeListener(_refresh);
    }
    _watched.clear();
    for (final root in widget.roots.all) {
      final watch = Listenable.merge([
        root.backend.workspace,
        Connection.of(root.backend),
      ]);
      _watched.add(watch);
      watch.addListener(_refresh);
    }
  }

  void _rootsChanged() {
    _watch();
    _refresh();
  }

  void _refresh() {
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    widget.roots.removeListener(_rootsChanged);
    for (final watch in _watched) {
      watch.removeListener(_refresh);
    }
    super.dispose();
  }

  Future<void> _choose(AndroidShareDestination target) async {
    if (_busy ||
        !target.exists ||
        !target.modelAvailable ||
        !widget.roots.all.contains(target.claktory) ||
        (target.isChat && widget.share.files.isNotEmpty)) {
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await target.select();
      // Cancel/retirement while the backend request was pending never transfers
      // or writes anything. A failed request leaves the picker retryable.
      if (!mounted) return;
      if (!widget.roots.all.contains(target.claktory) || !target.exists) {
        setState(() {
          _busy = false;
          _error = 'That conversation is gone. Choose another.';
        });
        return;
      }
      if (!target.modelAvailable) {
        setState(() {
          _busy = false;
          _error = 'No model available';
        });
        return;
      }
      widget.chosen(target);
    } catch (_) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = "Couldn't open that conversation. Try again.";
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final body = TextStyles.body.copyWith(color: palette.text);
    final share = widget.share;
    final destinations = androidShareDestinations(widget.roots);
    final loading = widget.roots.all.any(
      (root) => !root.backend.workspace.received,
    );
    final connections = {
      for (final root in widget.roots.all) root: Connection(root.backend),
    };
    final warning = androidShareError(share.error);
    return SizedBox(
      width: 440,
      height: MediaQuery.sizeOf(context).height * .65,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Expanded(
            child: ListView(
              children: [
                Text(
                  share.hasContent ? 'Send to…' : 'Nothing to add',
                  style: TextStyles.emphasis.copyWith(color: palette.text),
                ),
                const SizedBox(height: Space.m),
                if (share.hasContent)
                  Text(
                    'It goes into the message box. Nothing is sent until you tap Send.',
                    style: body,
                  ),
                if (share.hasContent) ...[
                  if (share.draftText.isNotEmpty)
                    Padding(
                      padding: const EdgeInsets.only(top: Space.m),
                      child: Text(
                        String.fromCharCodes(share.draftText.runes.take(500)),
                        maxLines: 3,
                        overflow: TextOverflow.ellipsis,
                        style: body,
                      ),
                    ),
                  for (final path in share.files)
                    Padding(
                      padding: const EdgeInsets.only(top: Space.s),
                      child: Text(path.split('/').last, style: body),
                    ),
                ],
                if (warning != null)
                  Padding(
                    padding: const EdgeInsets.only(top: Space.m),
                    child: Text(warning, style: body),
                  ),
                if (share.textCut)
                  Padding(
                    padding: const EdgeInsets.only(top: Space.m),
                    child: Text(
                      'Only the first 100,000 characters of the text are added.',
                      style: body,
                    ),
                  ),
                if (_error case final error?)
                  Padding(
                    padding: const EdgeInsets.only(top: Space.m),
                    child: Semantics(
                      liveRegion: true,
                      child: Text(
                        error,
                        style: body.copyWith(color: palette.danger),
                      ),
                    ),
                  ),
                if (share.hasContent) ...[
                  const SizedBox(height: Space.m),
                  if (_busy) Text('Loading…', style: body),
                  if (destinations.isEmpty && !loading)
                    Text(
                      widget.roots.all.isEmpty
                          ? 'Connect this $thisDeviceKind to your Claktory first.'
                          : 'No agents or chats yet.',
                      style: body,
                    ),
                  for (final MapEntry(key: root, value: connection)
                      in connections.entries) ...[
                    if (!root.backend.workspace.received ||
                        connection.link != Link.live ||
                        destinations.any(
                          (target) => identical(target.claktory, root),
                        ))
                      Padding(
                        padding: const EdgeInsets.symmetric(vertical: Space.m),
                        child: Text(
                          claktoryName(root.backend),
                          style: TextStyles.caption.copyWith(
                            color: palette.textMuted,
                          ),
                        ),
                      ),
                    if (!root.backend.workspace.received ||
                        connection.link != Link.live)
                      Semantics(
                        liveRegion: true,
                        child: Text(
                          connection.link == Link.live
                              ? 'Loading…'
                              : connection.line,
                          style: connection.problem
                              ? body.copyWith(color: palette.danger)
                              : body,
                        ),
                      ),
                    for (final target in destinations.where(
                      (target) => identical(target.claktory, root),
                    ))
                      _DestinationRow(
                        target: target,
                        disabled:
                            _busy ||
                            !target.modelAvailable ||
                            (target.isChat && share.files.isNotEmpty),
                        files: share.files.isNotEmpty,
                        choose: () => unawaited(_choose(target)),
                      ),
                  ],
                ],
              ],
            ),
          ),
          const SizedBox(height: Space.m),
          Button(
            label: share.hasContent ? 'Cancel' : 'Close',
            onPressed: widget.close,
          ),
        ],
      ),
    );
  }
}

class _DestinationRow extends StatelessWidget {
  const _DestinationRow({
    required this.target,
    required this.disabled,
    required this.files,
    required this.choose,
  });
  final AndroidShareDestination target;
  final bool disabled;
  final bool files;
  final VoidCallback choose;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.s),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          ConstrainedBox(
            constraints: const BoxConstraints(minHeight: 48),
            child: Button(
              label: target.name,
              onPressed: disabled ? null : choose,
            ),
          ),
          if (!target.modelAvailable)
            Padding(
              padding: const EdgeInsets.only(top: Space.xs),
              child: Text(
                'No model available',
                style: TextStyles.caption.copyWith(color: palette.textMuted),
              ),
            ),
          if (target.isChat && files)
            Padding(
              padding: const EdgeInsets.only(top: Space.xs),
              child: Text(
                'Chats take text only',
                style: TextStyles.caption.copyWith(color: palette.textMuted),
              ),
            ),
        ],
      ),
    );
  }
}
