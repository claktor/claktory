import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'connections.dart';
import 'drafts.dart';
import 'history.dart';
import 'identity.dart';
import 'layout.dart';
import 'login.dart';
import 'panes.dart' show pagePane, withPage;
import 'protocol/factory/v1/factory.pb.dart' show Agent;
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
enum AppPage { agent, settings, marketplace, changelog, map }

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
  static void openSettings(BuildContext context) =>
      openPage(context, AppPage.settings);

  /// Shows what changed in each version of Claktory.
  static void openChangelog(BuildContext context) =>
      openPage(context, AppPage.changelog);

  /// The agent whose layout [page]s open in: the selected one on a Mac;
  /// none on a phone, where they're pages of their own.
  static Agent? _besideAgent(BuildContext context) => isPhoneLayout(context)
      ? null
      : scope(context).backend.workspace.selectedAgent;

  /// Shows [page] (Settings, the Marketplace, What's new). On a Mac it's a
  /// pane right of the selected agent's conversation, so the two are read
  /// side by side; one already there glows instead. On a phone, or with no
  /// agent to show it beside, it takes the agent view's place.
  static void openPage(BuildContext context, AppPage page) {
    final scope = AppScope.scope(context);
    final agent = _besideAgent(context);
    if (agent == null) {
      scope.history.clear();
      scope.page.value = page;
      return;
    }
    if (scope.page.value != AppPage.agent) scope.page.value = AppPage.agent;
    final root = layoutOf(agent);
    final shown = pagePane(root, page);
    if (shown != null) {
      scope.backend.highlights.glow('pane:${agent.id}:${shown.id}', [
        agentColor(agent.color),
      ]);
      return;
    }
    fireAndLog(scope.backend.setLayout(agent.id, withPage(root, page)));
  }

  /// Whether [page] shows: as the agent view's page, or as a pane in the
  /// selected agent's layout.
  static bool pageShown(BuildContext context, AppPage page) {
    if (scope(context).page.value == page) return true;
    final agent = _besideAgent(context);
    return agent != null && pagePane(layoutOf(agent), page) != null;
  }

  /// The sidebar's buttons: [page] shows, or goes when it's showing.
  static void togglePage(BuildContext context, AppPage page) {
    final scope = AppScope.scope(context);
    if (scope.page.value == page) return closePage(context);
    final agent = _besideAgent(context);
    final shown = agent == null ? null : pagePane(layoutOf(agent), page);
    if (agent == null || shown == null) return openPage(context, page);
    final root = close(layoutOf(agent), shown.id);
    if (root == null) return;
    fireAndLog(
      scope.backend.setLayout(agent.id, isChatOnly(root) ? null : root),
    );
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
