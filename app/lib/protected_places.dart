// Places macOS keeps private until the person lets Claktory Agents in
// (Files and Folders: Documents, Desktop, Downloads, iCloud Drive,
// volumes). macOS has no way to ask
// whether an app may use one, so the backend keeps the last check's result
// (Workspace.protected_places); the app only says it and asks for checks.
//
// Before a project or folder starts working in one, and once for those that
// already did, the shared permission sheet says why ("Let your agents work
// in Documents") and its Continue has the backend read the places, so
// macOS's own prompt comes while the person is looking, never in the middle
// of an agent's task. Only on the Mac that runs the Claktory: its prompt
// shows on that Mac's screen.
import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:url_launcher/url_launcher.dart' show launchUrl;

import 'backend.dart';
import 'controls.dart' show FolderGlyph;
import 'mobile/permission.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';

/// "A", "A and B", "A, B and C".
String _and(List<String> names) => switch (names.length) {
  0 => '',
  1 => names.single,
  _ => '${names.sublist(0, names.length - 1).join(', ')} and ${names.last}',
};

/// Whether the Claktory [backend] shows runs on this Mac, where macOS's
/// prompt for its places shows.
bool protectedPlacesAskHere(Backend backend) =>
    defaultTargetPlatform == TargetPlatform.macOS &&
    backend.localBackend &&
    backend.target == null;

/// The places projects and folders work in that were never checked.
List<ProtectedPlace> uncheckedPlaces(Workspace workspace) => [
  for (final place in workspace.protectedPlaces)
    if (place.state == ProtectedPlaceState.PROTECTED_PLACE_STATE_UNSPECIFIED &&
        place.usedBy.isNotEmpty)
      place,
];

/// Opens System Settings at Privacy & Security → Files and Folders, or at
/// Privacy & Security where that pane's link has moved (the words always
/// give the whole way).
Future<void> openFilesAndFolders() async {
  final opened = await _open(
    Uri.parse(
      'x-apple.systempreferences:com.apple.preference.security'
      '?Privacy_FilesAndFolders',
    ),
  );
  if (!opened) {
    await _open(
      Uri.parse('x-apple.systempreferences:com.apple.preference.security'),
    );
  }
}

/// Opens [url]; false where it can't (a test, a pane that moved).
@visibleForTesting
Future<bool> Function(Uri url) openSystemSettings = launchUrl;

Future<bool> _open(Uri url) async {
  try {
    return await openSystemSettings(url);
  } on Object {
    return false;
  }
}

/// Files and Folders for [names] (as the person knows them: "Documents"),
/// where [usedBy] (projects and folders, by name) work. [check] has the
/// backend read them, macOS asking for any it hasn't decided on, and
/// answers what it found; null where nothing can be checked (the
/// Permissions rows, which only open Settings, with [read] saying denied).
class ProtectedPlacePermission extends Permission {
  const ProtectedPlacePermission({
    required this.names,
    required this.usedBy,
    this.check,
    this.read,
  });

  final List<String> names;
  final List<String> usedBy;
  final Future<PermissionAccess> Function()? check;

  /// What the backend last found, each time the sheet looks: undetermined
  /// (the Before stage) while never checked.
  final PermissionAccess Function()? read;

  @override
  PermissionWords words(BuildContext context) {
    final places = _and(names);
    final where = usedBy.isEmpty ? '' : ' to work in ${_and(usedBy)}';
    return PermissionWords(
      glyph: 'folder',
      // The project's folder glyph, at the sheet's size.
      drawGlyph: (color, size) => SizedBox.square(
        dimension: size,
        child: FittedBox(child: FolderGlyph(color: color)),
      ),
      title: 'Let your agents work in $places',
      body:
          'macOS keeps $places private until you let an app in. Your agents '
          'need ${names.length == 1 ? 'it' : 'them'}$where. Your Mac asks '
          'next, for Claktory Agents.',
      offTitle: "Your agents can't work in $places",
      offBody: usedBy.isEmpty
          ? "macOS hasn't let Claktory Agents into $places, so commands "
                'there fail.'
          : "macOS hasn't let Claktory Agents into $places, so commands in "
                '${_and(usedBy)} fail.',
      offCaption:
          'To turn it on: System Settings → Privacy & Security → Files and '
          'Folders → Claktory Agents.',
      openSettings: 'Open System Settings',
    );
  }

  @override
  Future<PermissionAccess> access() async =>
      read?.call() ?? PermissionAccess.undetermined;

  @override
  Future<PermissionAccess> request() async =>
      await check?.call() ?? PermissionAccess.unknown;

  @override
  Future<void> openSettings() => openFilesAndFolders();
}

/// Has [backend] read [roots] (macOS asking for any it hasn't decided on)
/// and answers what it found there: granted when every one let Claktory
/// Agents in, denied when any refused.
Future<PermissionAccess> checkProtectedPlaces(
  Backend backend,
  List<String> roots,
) async {
  // The Workspace follows the answer when anything changed: wait for it a
  // moment.
  final changed = Completer<void>();
  void listen() {
    if (!changed.isCompleted) changed.complete();
  }

  backend.workspace.addListener(listen);
  try {
    await backend.checkProtectedPlaces(roots);
    await changed.future.timeout(const Duration(seconds: 2), onTimeout: () {});
  } on RequestFailure {
    return PermissionAccess.unknown;
  } finally {
    backend.workspace.removeListener(listen);
  }
  final found = protectedPlacesAccess(backend.workspace.workspace, roots);
  return found == PermissionAccess.undetermined
      ? PermissionAccess.unknown
      : found;
}

/// What the backend last found at [roots]: denied when any refused,
/// granted when every one let Claktory Agents in, else undetermined.
PermissionAccess protectedPlacesAccess(
  Workspace workspace,
  List<String> roots,
) {
  final found = [
    for (final place in workspace.protectedPlaces)
      if (roots.contains(place.root)) place.state,
  ];
  if (found.contains(ProtectedPlaceState.PROTECTED_PLACE_STATE_REFUSED)) {
    return PermissionAccess.denied;
  }
  return found.isNotEmpty &&
          found.every(
            (state) =>
                state == ProtectedPlaceState.PROTECTED_PLACE_STATE_ALLOWED,
          )
      ? PermissionAccess.granted
      : PermissionAccess.undetermined;
}

/// The sheet for [places], then the check its Continue asks for. Answers
/// what was found.
Future<PermissionAccess> askProtectedPlaces(
  BuildContext context,
  List<ProtectedPlace> places,
) {
  final backend = AppScope.of(context);
  final roots = [for (final place in places) place.root];
  return askPermission(
    context,
    ProtectedPlacePermission(
      names: [for (final place in places) place.name],
      usedBy: {for (final place in places) ...place.usedBy}.toList(),
      check: () => checkProtectedPlaces(backend, roots),
      read: () => protectedPlacesAccess(backend.workspace.workspace, roots),
    ),
  );
}

/// The places asked about in this session, for each Claktory, at launch or by a form saving a
/// folder into one, so the sheet comes once for each; kept only while the
/// app runs.
final _askedBy = Expando<Set<String>>();

Set<String> _asked(Backend backend) => _askedBy[backend] ??= {};

/// The place macOS protects that [path] is in, by whole components (as
/// factoryd decides): the place's root itself, or anything under it. Null
/// when it's in none, and always on a Claktory that doesn't run on a Mac.
ProtectedRoot? protectedRootOf(Workspace workspace, String path) {
  var typed = path.trim();
  while (typed.length > 1 && typed.endsWith('/')) {
    typed = typed.substring(0, typed.length - 1);
  }
  if (typed.isEmpty) return null;
  for (final place in workspace.protectedRoots) {
    final root = place.root;
    if (typed == root ||
        typed.startsWith(root.endsWith('/') ? root : '$root/')) {
      return place;
    }
  }
  return null;
}

/// What the backend last found at [root]: unspecified while never checked.
ProtectedPlaceState protectedPlaceState(Workspace workspace, String root) =>
    [
      for (final place in workspace.protectedPlaces)
        if (place.root == root) place.state,
    ].firstOrNull ??
    ProtectedPlaceState.PROTECTED_PLACE_STATE_UNSPECIFIED;

/// [place] as a sentence names it: "your Documents folder", "your iCloud
/// Drive", "the Backup disk".
String _placeInWords(ProtectedRoot place) {
  if (place.root.startsWith('/Volumes/')) return 'the ${place.name} disk';
  if (place.name == 'iCloud Drive') return 'your iCloud Drive';
  return 'your ${place.name} folder';
}

/// The caption under a folder's path typed into [place]:
/// macOS asks first while it's never been checked, and says how to turn it
/// on once it was refused. Null once Claktory Agents is let in.
String? protectedPathCaption(Workspace workspace, ProtectedRoot place) =>
    switch (protectedPlaceState(workspace, place.root)) {
      ProtectedPlaceState.PROTECTED_PLACE_STATE_ALLOWED => null,
      ProtectedPlaceState.PROTECTED_PLACE_STATE_REFUSED =>
        "macOS hasn't let Claktory Agents into ${_placeInWords(place)}. To "
            'turn it on: System Settings → Privacy & Security → Files and '
            'Folders → Claktory Agents.',
      _ => 'macOS asks before Claktory Agents can use ${_placeInWords(place)}.',
    };

/// Saves folders with [save], [folders] mapping each one's path to its
/// name. On the Mac that runs the Claktory, when any goes into a place
/// never checked, the sheet comes first ("Let your agents work in
/// Documents"): its Continue saves, which has the backend read the folder,
/// so macOS asks then, while the person is looking; then it reads what
/// macOS answered. Answers false when the person set the sheet aside and
/// nothing was saved; a refused save throws its [RequestFailure].
Future<bool> saveIntoProtectedPlaces(
  BuildContext context, {
  required Map<String, String> folders,
  required Future<void> Function() save,
}) async {
  final backend = AppScope.of(context);
  final workspace = backend.workspace.workspace;
  final places = <String, ProtectedRoot>{};
  final usedBy = <String>[];
  for (final MapEntry(key: path, value: name) in folders.entries) {
    final place = protectedRootOf(workspace, path);
    if (place == null ||
        protectedPlaceState(workspace, place.root) !=
            ProtectedPlaceState.PROTECTED_PLACE_STATE_UNSPECIFIED) {
      continue;
    }
    places[place.root] = place;
    usedBy.add(name);
  }
  if (places.isEmpty || !protectedPlacesAskHere(backend)) {
    await save();
    return true;
  }
  final roots = places.keys.toList();
  // The launch's sheet leaves them to this one.
  _asked(backend).addAll(roots);
  var saved = false;
  RequestFailure? refused;
  await askPermission(
    context,
    ProtectedPlacePermission(
      names: [for (final place in places.values) place.name],
      usedBy: usedBy,
      check: () async {
        try {
          await save();
          saved = true;
        } on RequestFailure catch (failure) {
          refused = failure;
          return PermissionAccess.unknown;
        }
        // The backend read the folder as it saved it, so macOS has answered:
        // read the places again for what it said.
        return checkProtectedPlaces(backend, roots);
      },
      read: () => protectedPlacesAccess(backend.workspace.workspace, roots),
    ),
    // A save can wait: from the start, Not now goes back to the form with
    // nothing saved.
    wayOut: PermissionWayOut('Not now', () {}),
  );
  if (refused case final failure?) throw failure;
  return saved;
}

/// Once a session, on the Mac that runs the Claktory: the projects and
/// folders that already work in a protected place never checked (from
/// before this, or added on another device) get the sheet, listing them
/// all, and its Continue has the backend read them. Wraps [child], which
/// it shows as it is.
class ProtectedPlacesAtLaunch extends StatefulWidget {
  const ProtectedPlacesAtLaunch({super.key, required this.child});

  final Widget child;

  @override
  State<ProtectedPlacesAtLaunch> createState() =>
      _ProtectedPlacesAtLaunchState();
}

class _ProtectedPlacesAtLaunchState extends State<ProtectedPlacesAtLaunch> {
  var _asking = false;
  Backend? _backend;

  /// Back at the front: the refused places read again, so one let in from
  /// System Settings meanwhile shows On, and its Needs you item ends.
  /// Reading a place macOS has decided on
  /// asks nothing.
  late final _resumed = AppLifecycleListener(onResume: _recheck);

  @override
  void initState() {
    super.initState();
    _resumed;
  }

  void _recheck() {
    final backend = _backend;
    if (backend == null || !protectedPlacesAskHere(backend)) return;
    final refused = [
      for (final place in backend.workspace.workspace.protectedPlaces)
        if (place.state == ProtectedPlaceState.PROTECTED_PLACE_STATE_REFUSED)
          place.root,
    ];
    if (refused.isNotEmpty) fireAndLog(backend.checkProtectedPlaces(refused));
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final backend = AppScope.of(context);
    if (!identical(backend, _backend)) {
      _backend?.workspace.removeListener(_changed);
      _backend = backend..workspace.addListener(_changed);
      _changed();
    }
  }

  @override
  void dispose() {
    _resumed.dispose();
    _backend?.workspace.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    final backend = _backend;
    if (backend == null || _asking || !protectedPlacesAskHere(backend)) {
      return;
    }
    final places = [
      for (final place in uncheckedPlaces(backend.workspace.workspace))
        if (!_asked(backend).contains(place.root)) place,
    ];
    if (places.isEmpty) return;
    _asking = true;
    _asked(backend).addAll(places.map((place) => place.root));
    // After this frame: the workspace changes while it's built.
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      try {
        if (mounted) await askProtectedPlaces(context, places);
      } finally {
        _asking = false;
      }
    });
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
