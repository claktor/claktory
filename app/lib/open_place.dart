import 'package:clock/clock.dart';
import 'package:flutter/widgets.dart';

import 'app_dock.dart' show closeExpandedApp;
import 'backend.dart';
import 'checklist_reply.dart' show showChecklistItem;
import 'ipad/workbench.dart' show WorkbenchScope;
import 'layout.dart' show chatKind, layoutOf, leaves;
import 'mobile/navigation.dart' show PhoneRoutes;
import 'monitors_overview.dart' show monitorKey;
import 'panes.dart' show pageKinds;
import 'protocol/factory/v1/factory.pb.dart' show MonitorState, ViewNode;
import 'screens.dart' show ScreenOpener;
import 'tasks.dart' show taskKey;
import 'scope.dart';

/// A place in Claktory that an entry elsewhere opens: a calendar's entry,
/// a row of Device activity.
sealed class OpenTarget {
  const OpenTarget();
}

/// A place that belongs to an agent, gone with it.
sealed class AgentTarget extends OpenTarget {
  const AgentTarget(this.agent);

  final String agent;
}

/// [agent]'s conversation.
final class OpenAgent extends AgentTarget {
  const OpenAgent(super.agent);
}

/// Item [item] (#3 is 3) of [agent]'s checklist.
final class OpenItem extends AgentTarget {
  const OpenItem(super.agent, this.item);

  final int item;
}

/// [agent]'s monitor [monitor] (m3 is 3), on the Monitors page.
final class OpenMonitor extends AgentTarget {
  const OpenMonitor(super.agent, this.monitor);

  final int monitor;
}

/// [agent]'s background task [task] ("task-3"), in its Tasks.
final class OpenTask extends AgentTarget {
  const OpenTask(super.agent, this.task);

  final String task;
}

/// Why [target] can't be opened now, in a sentence ("This item was
/// deleted."), or null when it can.
Future<String?> whyGone(Backend backend, OpenTarget target) async {
  if (target is AgentTarget && backend.workspace.agent(target.agent) == null) {
    return 'This agent was deleted.';
  }
  switch (target) {
    case OpenAgent():
      return null;
    case OpenItem(:final agent, :final item):
      final checklist = await backend.getChecklist(agent);
      final found = checklist?.items.any((entry) => entry.id == item) ?? false;
      return found ? null : 'This item was deleted.';
    case OpenMonitor(:final agent, :final monitor):
      final list = await backend.listMonitors(agentId: agent);
      // A cancelled one stays listed for a while, but has nothing to open.
      final found = list.monitors.any(
        (entry) =>
            entry.agentId == agent &&
            entry.monitor.id == monitor &&
            entry.monitor.state != MonitorState.MONITOR_STATE_CANCELLED,
      );
      return found ? null : 'This monitor was cancelled.';
    case OpenTask(:final agent, :final task):
      final tasks = await backend.listTasks(agent);
      final found =
          tasks?.tasks.any(
            (entry) => entry.agentId == agent && entry.id == task,
          ) ??
          false;
      return found ? null : 'This task has ended.';
  }
}

/// Goes to [target]'s place, over whatever page is open. An app's view
/// filling the window closes first, as its Back would, and an iPad's
/// sidebar over the window goes, as for anything chosen there; on a phone
/// a conversation is pushed over what shows, so Back returns to it.
void openTarget(BuildContext context, OpenTarget target) {
  context = closeExpandedApp(context);
  final workbench = WorkbenchScope.maybeOf(context)?..closeOverlay();
  final backend = AppScope.of(context);
  final phone = PhoneRoutes.of(context);
  void point(String key) =>
      backend.highlights.pointedAt.value = (keys: [key], at: clock.now());
  // On an iPad, a pane already in the layout as a tab behind another comes
  // to the front.
  void front(String agentId, bool Function(ViewNode leaf) which) {
    final agent = backend.workspace.agent(agentId);
    if (workbench == null || agent == null) return;
    final leaf = leaves(layoutOf(agent)).where(which).firstOrNull;
    if (leaf != null) workbench.focus(agentId, leaf.id);
  }

  switch (target) {
    case OpenAgent(:final agent):
      if (phone?.pushConversation(agent) ?? false) return;
      front(agent, (leaf) => leaf.leaf.kind == chatKind);
      ScreenOpener.openConversation(context, agent);
    case OpenItem(:final agent, :final item):
      if (phone != null) {
        point('checklist_item:$agent#$item');
        if (phone.pushConversation(agent, panel: 'checklist')) return;
      }
      AppScope.closePage(context);
      fireAndLog(showChecklistItem(backend, agent, item));
    case OpenMonitor(:final agent, :final monitor):
      // Pointed at first, so the page goes to it however it opens.
      point(monitorKey(agent, monitor));
      final selected = backend.workspace.selectedAgent?.id;
      if (selected != null) {
        front(
          selected,
          (leaf) => leaf.leaf.kind == pageKinds[AppPage.monitors],
        );
      }
      if (!AppScope.pageShown(context, AppPage.monitors)) {
        AppScope.openPage(context, AppPage.monitors);
      }
    case OpenTask(:final agent, :final task):
      point(taskKey(agent, task));
      if (phone?.pushConversation(agent, panel: 'tasks') ?? false) return;
      ScreenOpener.openConversation(context, agent);
      fireAndLog(backend.openPanel(agent, 'tasks'));
  }
}

/// What app views (a calendar's entries) open their places with: provided
/// above them by the window that can go there. None above: they offer no
/// way to open.
class OpenPlace extends InheritedWidget {
  const OpenPlace({
    super.key,
    required this.gone,
    required this.open,
    required super.child,
  });

  /// Why a target can't be opened now ([whyGone]), or null when it can.
  final Future<String?> Function(OpenTarget target) gone;

  /// Goes to a target's place ([openTarget]).
  final void Function(BuildContext context, OpenTarget target) open;

  static OpenPlace? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<OpenPlace>();

  @override
  bool updateShouldNotify(OpenPlace oldWidget) =>
      gone != oldWidget.gone || open != oldWidget.open;
}
