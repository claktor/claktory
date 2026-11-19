import 'dart:async';

import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'forms.dart';
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';

/// Lets the person pick a folder on the machine a Factory runs on, which
/// this Mac's folder sheet can't show: it opens at [from] (or the home
/// folder when that can't be read) and answers the folder chosen, or null.
Future<String?> chooseRemoteFolder(
  BuildContext context, {
  required Backend backend,
  required String from,
  required String machine,
}) async {
  String? chosen;
  await showModal(
    context,
    (context, close) => FolderBrowser(
      backend: backend,
      from: from,
      machine: machine,
      onChoose: (path) {
        chosen = path;
        close();
      },
      onCancel: close,
    ),
  );
  return chosen;
}

/// The browser in [chooseRemoteFolder]: breadcrumbs of where it is, Up,
/// Home and Show hidden, and the folders here; a click opens one, and
/// Choose takes the folder it's in.
class FolderBrowser extends StatefulWidget {
  const FolderBrowser({
    super.key,
    required this.backend,
    required this.from,
    required this.machine,
    required this.onChoose,
    required this.onCancel,
  });

  final Backend backend;

  /// Where it opens: a full path, or one from the home folder ("~/code").
  final String from;
  final String machine;
  final ValueChanged<String> onChoose;
  final VoidCallback onCancel;

  @override
  State<FolderBrowser> createState() => _FolderBrowserState();
}

class _FolderBrowserState extends State<FolderBrowser> {
  FolderList? _list;
  String? _error;
  var _showHidden = false;

  /// Only the latest request's answer shows.
  var _asked = 0;

  @override
  void initState() {
    super.initState();
    final from = widget.from.trim();
    unawaited(
      _open(
        from.startsWith('/') || from.startsWith('~') ? from : '~',
        fallBack: true,
      ),
    );
  }

  Future<void> _open(String path, {bool fallBack = false}) async {
    final asked = ++_asked;
    try {
      final list = await widget.backend.listFolders(path);
      if (!mounted || asked != _asked) return;
      setState(() {
        _list = list;
        _error = null;
      });
    } on RequestFailure catch (failure) {
      if (!mounted || asked != _asked) return;
      // A path typed for a folder that doesn't exist yet opens at home.
      if (fallBack && path != '~') return _open('~');
      setState(() => _error = failure.message);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final list = _list;
    final folders = [
      if (list != null)
        for (final folder in list.folders)
          if (_showHidden || !folder.hidden) folder,
    ];
    return FormFrame(
      title: 'Choose a folder',
      body: widget.machine.isEmpty
          ? 'On the machine this Claktory runs on.'
          : 'On ${widget.machine}, where this Claktory runs.',
      error: _error,
      submitLabel: 'Choose',
      onSubmit: list == null ? null : () => widget.onChoose(list.path),
      onCancel: widget.onCancel,
      fields: [
        if (list != null) _Crumbs(list: list, onOpen: _open),
        const SizedBox(height: Space.s),
        Row(
          children: [
            Button(
              label: 'Up',
              onPressed: list == null || list.parent.isEmpty
                  ? null
                  : () => _open(list.parent),
            ),
            const SizedBox(width: Space.xs),
            Button(
              label: 'Home',
              onPressed: list == null || list.path == list.home
                  ? null
                  : () => _open(list.home.isEmpty ? '~' : list.home),
            ),
            const Spacer(),
            Text(
              'Show hidden',
              style: TextStyles.caption.copyWith(color: palette.textMuted),
            ),
            const SizedBox(width: Space.s),
            Toggle(
              value: _showHidden,
              label: 'Show hidden',
              onChanged: (value) => setState(() => _showHidden = value),
            ),
          ],
        ),
        const SizedBox(height: Space.s),
        Container(
          height: 280,
          decoration: BoxDecoration(
            color: palette.sidebar,
            border: Border.all(color: palette.hairline),
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: list == null
              ? null
              : folders.isEmpty
              ? Padding(
                  padding: const EdgeInsets.all(Space.m),
                  child: Text(
                    'No folders here.',
                    style: TextStyles.body.copyWith(color: palette.textFaint),
                  ),
                )
              : ListView.builder(
                  key: ValueKey(list.path),
                  padding: const EdgeInsets.all(Space.xs),
                  itemCount: folders.length,
                  itemBuilder: (context, index) {
                    final folder = folders[index];
                    return _FolderRow(
                      folder: folder,
                      onOpen: () => _open(
                        list.path == '/'
                            ? '/${folder.name}'
                            : '${list.path}/${folder.name}',
                      ),
                    );
                  },
                ),
        ),
        if (list != null && list.truncated)
          Padding(
            padding: const EdgeInsets.only(top: Space.xs),
            child: Text(
              'Only the first 5,000 folders here are shown.',
              style: TextStyles.caption.copyWith(color: palette.textFaint),
            ),
          ),
      ],
    );
  }
}

/// Where the browser is, a crumb per folder from the root ("/") or the home
/// folder ("~"); each but the last opens its folder.
class _Crumbs extends StatelessWidget {
  const _Crumbs({required this.list, required this.onOpen});

  final FolderList list;
  final ValueChanged<String> onOpen;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final crumbs = folderCrumbs(list.path, list.home);
    return Wrap(
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        for (var i = 0; i < crumbs.length; i++) ...[
          if (i > 0 && crumbs[i - 1].$1 != '/')
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 2),
              child: Text(
                '/',
                style: TextStyles.body.copyWith(color: palette.textFaint),
              ),
            ),
          i == crumbs.length - 1
              ? Text(
                  crumbs[i].$1,
                  style: TextStyles.label.copyWith(color: palette.text),
                )
              : Pressable(
                  onTap: () => onOpen(crumbs[i].$2),
                  builder: (context, hovered) => Text(
                    crumbs[i].$1,
                    style: TextStyles.body.copyWith(
                      color: hovered ? palette.text : palette.textMuted,
                    ),
                  ),
                ),
        ],
      ],
    );
  }
}

/// The crumbs of [path], each (its name, its path): from "~" when it's in
/// [home], from "/" otherwise.
List<(String, String)> folderCrumbs(String path, String home) {
  final inHome =
      home.isNotEmpty &&
      home != '/' &&
      (path == home || path.startsWith('$home/'));
  final crumbs = <(String, String)>[inHome ? ('~', home) : ('/', '/')];
  var at = inHome ? home : '';
  final rest = inHome ? path.substring(home.length) : path;
  for (final name in rest.split('/')) {
    if (name.isEmpty) continue;
    at = '$at/$name';
    crumbs.add((name, at));
  }
  return crumbs;
}

class _FolderRow extends StatelessWidget {
  const _FolderRow({required this.folder, required this.onOpen});

  final FolderEntry folder;
  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final color = folder.hidden ? palette.textFaint : palette.text;
    return Pressable(
      onTap: onOpen,
      builder: (context, hovered) => Container(
        height: 28,
        padding: const EdgeInsets.symmetric(horizontal: Space.s),
        decoration: BoxDecoration(
          color: hovered ? palette.hover : null,
          borderRadius: BorderRadius.circular(Radii.control),
        ),
        child: Row(
          children: [
            FolderGlyph(
              color: folder.hidden ? palette.textFaint : palette.textMuted,
            ),
            const SizedBox(width: Space.s),
            Expanded(
              child: Text(
                folder.name,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyles.body.copyWith(color: color),
              ),
            ),
            Chevron(color: palette.textFaint, down: false),
          ],
        ),
      ),
    );
  }
}
