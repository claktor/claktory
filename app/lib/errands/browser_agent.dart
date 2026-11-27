import 'package:flutter/widgets.dart';

import '../backend.dart' show Backend;
import '../mobile/presentation.dart' show SheetAction, showActions;
import '../protocol/factory/v1/factory.pb.dart';
import '../state.dart' show isAdminAgent;
import 'origin.dart';

const browserExtension = 'browser';

/// Why [agent] can't have the browser; null when it can. Page text beside
/// an admin's powers, or a limited agent's, would widen what a page can
/// make it do, so the backend refuses the same.
String? browserRefusal(Agent agent) {
  if (isAdminAgent(agent)) return "Admin agents can't use the browser.";
  if (agent.quarantined) {
    return "Agents limited to a pipe can't use the browser.";
  }
  if (agent.readsReports) {
    return "Agents that read reports can't use the browser.";
  }
  return null;
}

String browserDescription(String name) =>
    "Lets $name use websites in Claktory's browser, signing in with your "
    'logins from Secrets.';

bool hasBrowser(Agent agent) => agent.extensions.any(
  (extension) => extension.id == browserExtension && extension.enabled,
);

AgentBrowser? browserOf(Workspace workspace, String agentId) {
  for (final browser in workspace.agentBrowsers) {
    if (browser.agentId == agentId) return browser;
  }
  return null;
}

List<String> signedInHosts(Workspace workspace, String agentId) {
  final hosts = <String>[];
  for (final site in browserOf(workspace, agentId)?.signedInSites ?? const []) {
    final host = siteName(site);
    if (!hosts.contains(host)) hosts.add(host);
  }
  return hosts;
}

String siteName(String origin) {
  final host = originHost(origin);
  return host.startsWith('www.') ? host.substring(4) : host;
}

String sitesText(List<String> hosts) {
  if (hosts.length > 4) {
    return '${hosts.take(4).join(', ')} and ${hosts.length - 4} more';
  }
  if (hosts.length < 2) return hosts.join();
  return '${hosts.take(hosts.length - 1).join(', ')} and ${hosts.last}';
}

String signedInText(List<String> hosts) => hosts.isEmpty
    ? 'Not signed in anywhere'
    : 'Signed in to ${sitesText(hosts)}';

/// Whether the person is sure: turning the browser off signs [agent] out
/// everywhere and deletes its browser's data.
Future<bool> confirmBrowserOff(
  BuildContext context,
  Backend backend,
  Agent agent,
) async {
  final hosts = signedInHosts(backend.workspace.workspace, agent.id);
  final sure = await showActions<bool>(
    context,
    title: "Turn off ${agent.name}'s browser?",
    message: hosts.isEmpty
        ? "${agent.name}'s browser data is deleted. Your logins stay in "
              'Secrets.'
        : '${agent.name} is signed out of ${sitesText(hosts)}, and the '
              "browser's data is deleted. Your logins stay in Secrets.",
    actions: const [SheetAction('Turn off', true, destructive: true)],
  );
  return sure == true;
}

/// Turns [agent]'s browser off once the person is sure; completes with
/// whether it was.
Future<bool> turnOffBrowser(
  BuildContext context,
  Backend backend,
  Agent agent,
) async {
  if (!await confirmBrowserOff(context, backend, agent)) return false;
  await backend.updateAgent(
    UpdateAgent(
      agentId: agent.id,
      extension_8: ExtensionChange(id: browserExtension, enabled: false),
    ),
  );
  return true;
}
