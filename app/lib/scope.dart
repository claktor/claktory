import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'connections.dart';
import 'drafts.dart';
import 'history.dart';
import 'login.dart';
import 'scanner.dart';
import 'update.dart';

/// On a phone, whether an agent's conversation is pushed over the sidebar:
/// how the app moves between its screens there (docs/IDENTITY.md, "Phone").
/// Local to the screen, like [AppPage].
class PhoneNavigation extends InheritedNotifier<ValueNotifier<bool>> {
  const PhoneNavigation({
    super.key,
    required ValueNotifier<bool> conversation,
    required super.child,
  }) : super(notifier: conversation);

  /// Null outside a phone's layout.
  static ValueNotifier<bool>? of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<PhoneNavigation>()?.notifier;
}

/// What the content area shows.
enum AppPage { agent, settings, marketplace, changelog }

/// What widgets below the app share: the backend with its stores, the
/// browser side of sign-in, which page is showing, this app's own build and
/// the unsent drafts kept on this device.
class AppScope extends InheritedWidget {
  const AppScope({
    super.key,
    required this.backend,
    required this.login,
    required this.page,
    required this.appBuild,
    required this.drafts,
    required this.history,
    required this.connections,
    this.camera = const DeviceCamera(),
    required super.child,
  });

  final Backend backend;
  final LoginLauncher login;

  /// Settings or the Marketplace replace the agent view while showing. Which
  /// page a window shows is local to it; everything on it is the backend's
  /// state.
  final ValueNotifier<AppPage> page;

  final AppBuild appBuild;
  final Drafts drafts;

  /// Where the person came from, following agents' messages.
  final AgentHistory history;

  /// Which Factory this app shows, kept on this device.
  final Connections connections;

  /// The phone's camera, which scans pairing codes.
  final Camera camera;

  static AppScope scope(BuildContext context) {
    final scope = context.getInheritedWidgetOfExactType<AppScope>();
    assert(scope != null, 'No AppScope above this widget.');
    return scope!;
  }

  static Backend of(BuildContext context) => scope(context).backend;

  /// Shows Settings, where providers are signed in.
  static void openSettings(BuildContext context) {
    scope(context).history.clear();
    scope(context).page.value = AppPage.settings;
  }

  /// Shows what changed in each version of Factory.
  static void openChangelog(BuildContext context) {
    scope(context).history.clear();
    scope(context).page.value = AppPage.changelog;
  }

  /// Shows [target]'s Factory from now on (null for this Mac's): the
  /// device remembers it, and connects to it if it's new. Every Factory
  /// it knows stays connected; the window shows this one, as it was left.
  static void switchFactory(BuildContext context, RemoteFactory? target) =>
      scope(context).connections.use(target);

  /// Back to the selected agent, or on to one the person picked: either
  /// way, a fresh start for back and forward.
  static void closePage(BuildContext context) {
    scope(context).history.clear();
    scope(context).page.value = AppPage.agent;
  }

  @override
  bool updateShouldNotify(AppScope oldWidget) =>
      oldWidget.backend != backend ||
      oldWidget.login != login ||
      oldWidget.page != page ||
      oldWidget.appBuild != appBuild ||
      oldWidget.drafts != drafts ||
      oldWidget.history != history ||
      oldWidget.camera != camera ||
      oldWidget.connections != connections;
}
