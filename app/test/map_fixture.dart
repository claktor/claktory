// A ClaktoryMap built the way the backend builds one (crates/factoryd/src/
// map.rs), for tests and renders.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';

class FakeAgent {
  const FakeAgent(
    this.id,
    this.name, {
    this.role = '',
    this.color = 'slate',
    this.project = '',
    this.folder = '',
    this.parent = '',
    this.admin = false,
    this.reachable = true,
    this.also = const [],
    this.badges = const ['web', 'tree'],
    this.records = false,
  });

  final String id;
  final String name;
  final String role;
  final String color;
  final String project;
  final String folder;
  final String parent;
  final bool admin;
  final bool reachable;
  final List<String> also;
  final List<String> badges;
  final bool records;
}

class FakeProject {
  const FakeProject(this.id, this.name, {this.folders = const []});

  final String id;
  final String name;
  final List<String> folders;
}

class FakeSecret {
  const FakeSecret(this.id, this.name, this.holders);

  final String id;
  final String name;
  final List<String> holders;
}

class FakeApp {
  const FakeApp(
    this.id,
    this.name, {
    this.icon = '',
    this.given = const {},
    this.hears = const {},
  });

  final String id;
  final String name;
  final String icon;

  /// Agent id → the scopes' labels, joined.
  final Map<String, String> given;

  /// Agent id → the events' labels, joined.
  final Map<String, String> hears;
}

const _badgeLabels = {
  'agents:admin': ('Messages every agent', 'users'),
  'secrets:give': ('Gives secrets to agents', 'key-round'),
  'tasks:see_all': ("Stops any agent's tasks", 'terminal'),
  'web': ('Web', 'globe'),
  'tree': ('Own checkout', 'git-branch'),
  'database': ('Database', 'database'),
  'database:factory': ("Claktory's records", 'database'),
  'memory:factory': ('Claktory manual', 'brain'),
};

ClaktoryMap fakeMap({
  List<FakeProject> projects = const [],
  List<FakeAgent> agents = const [],
  List<FakeSecret> secrets = const [],
  List<FakeApp> apps = const [],
  Map<String, String> leads = const {},
}) {
  final nodes = <MapNode>[
    MapNode(
      id: 'workspace',
      kind: MapNodeKind.MAP_NODE_KIND_WORKSPACE,
      label: 'Workspace',
      detail: '/Users/me/.factory/workspace',
    ),
  ];
  final edges = <MapEdge>[];
  for (final project in projects) {
    nodes.add(
      MapNode(
        id: 'project:${project.id}',
        kind: MapNodeKind.MAP_NODE_KIND_PROJECT,
        label: project.name,
        detail: '/Users/me/${project.id}',
        refId: project.id,
      ),
    );
    for (final folder in project.folders) {
      nodes.add(
        MapNode(
          id: 'folder:${project.id}/${folder.toLowerCase()}',
          kind: MapNodeKind.MAP_NODE_KIND_FOLDER,
          label: folder,
          detail: '/Users/me/${project.id}/${folder.toLowerCase()}',
          parentId: 'project:${project.id}',
          refId: folder.toLowerCase(),
          projectId: project.id,
        ),
      );
    }
  }
  String place(FakeAgent agent) => agent.project.isEmpty
      ? 'workspace'
      : agent.folder.isEmpty
      ? 'project:${agent.project}'
      : 'folder:${agent.project}/${agent.folder}';
  bool canMessage(FakeAgent from, FakeAgent to) {
    if (from.id == to.id || !to.reachable) return false;
    if (from.admin) return true;
    if (from.project == to.project) return true;
    return from.also.contains(to.id);
  }

  var records = false;
  for (final agent in agents) {
    final id = 'agent:${agent.id}';
    nodes.add(
      MapNode(
        id: id,
        kind: MapNodeKind.MAP_NODE_KIND_AGENT,
        label: agent.name,
        detail: agent.role,
        parentId: agent.admin
            ? ''
            : agent.parent.isNotEmpty
            ? 'agent:${agent.parent}'
            : place(agent),
        admin: agent.admin,
        color: agent.color,
        badges: [
          for (final key in [
            if (agent.admin) 'agents:admin',
            ...agent.badges,
            if (agent.records) 'database:factory',
          ])
            MapBadge(
              key: key,
              label: _badgeLabels[key]!.$1,
              icon: _badgeLabels[key]!.$2,
            ),
        ],
        refId: agent.id,
        projectId: agent.project,
      ),
    );
    edges.add(
      MapEdge(
        fromId: id,
        toId: place(agent),
        kind: MapEdgeKind.MAP_EDGE_KIND_WRITES,
      ),
    );
    if (agent.project.isNotEmpty) {
      final project = projects.firstWhere((p) => p.id == agent.project);
      for (final other in [
        'project:${project.id}',
        for (final folder in project.folders)
          'folder:${project.id}/${folder.toLowerCase()}',
      ]) {
        if (other == place(agent)) continue;
        edges.add(
          MapEdge(
            fromId: id,
            toId: other,
            kind: MapEdgeKind.MAP_EDGE_KIND_READS,
          ),
        );
      }
    }
    if (agent.records) {
      records = true;
      edges.add(
        MapEdge(
          fromId: id,
          toId: 'data:records',
          kind: MapEdgeKind.MAP_EDGE_KIND_READS,
        ),
      );
    }
    for (final to in agents) {
      if (!canMessage(agent, to)) continue;
      final back = canMessage(to, agent);
      if (back && to.id.compareTo(agent.id) < 0) continue;
      edges.add(
        MapEdge(
          fromId: id,
          toId: 'agent:${to.id}',
          kind: MapEdgeKind.MAP_EDGE_KIND_MESSAGES,
          bothWays: back,
        ),
      );
    }
  }
  for (final MapEntry(key: agent, value: lead) in leads.entries) {
    edges.add(
      MapEdge(
        fromId: 'agent:$agent',
        toId: 'agent:$lead',
        kind: MapEdgeKind.MAP_EDGE_KIND_LEAD,
      ),
    );
  }
  if (records) {
    nodes.add(
      MapNode(
        id: 'data:records',
        kind: MapNodeKind.MAP_NODE_KIND_DATA,
        label: "Claktory's records",
        detail: '/Users/me/.factory/factory.db',
      ),
    );
  }
  for (final secret in secrets) {
    nodes.add(
      MapNode(
        id: 'secret:${secret.id}',
        kind: MapNodeKind.MAP_NODE_KIND_SECRET,
        label: secret.name,
        refId: secret.id,
      ),
    );
    for (final holder in secret.holders) {
      edges.add(
        MapEdge(
          fromId: 'secret:${secret.id}',
          // A secret given to an app goes to the app.
          toId: holder.startsWith('app:') ? holder : 'agent:$holder',
          kind: MapEdgeKind.MAP_EDGE_KIND_SECRET,
        ),
      );
    }
  }
  for (final app in apps) {
    nodes.add(
      MapNode(
        id: 'app:${app.id}',
        kind: MapNodeKind.MAP_NODE_KIND_APP,
        label: app.name,
        refId: app.id,
        icon: app.icon,
      ),
    );
    for (final MapEntry(key: agent, value: label) in app.given.entries) {
      edges.add(
        MapEdge(
          fromId: 'app:${app.id}',
          toId: 'agent:$agent',
          kind: MapEdgeKind.MAP_EDGE_KIND_APP,
          label: label,
        ),
      );
    }
    for (final MapEntry(key: agent, value: label) in app.hears.entries) {
      edges.add(
        MapEdge(
          fromId: 'app:${app.id}',
          toId: 'agent:$agent',
          kind: MapEdgeKind.MAP_EDGE_KIND_HEARS,
          label: label,
        ),
      );
    }
  }
  nodes.sort((a, b) => a.id.compareTo(b.id));
  edges.sort((a, b) {
    final from = a.fromId.compareTo(b.fromId);
    if (from != 0) return from;
    final to = a.toId.compareTo(b.toId);
    return to != 0 ? to : a.kind.value.compareTo(b.kind.value);
  });
  return ClaktoryMap(nodes: nodes, edges: edges);
}

const claktoryProjects = [
  FakeProject('factory', 'Claktory', folders: ['App', 'Research']),
  FakeProject('disk', 'Disk'),
];

const claktoryAgents = [
  FakeAgent(
    'rivet',
    'Rivet',
    role: 'Admin',
    color: 'blue',
    admin: true,
    reachable: false,
    badges: ['secrets:give', 'tasks:see_all', 'web', 'tree'],
    records: true,
  ),
  FakeAgent(
    'qa',
    'Nora',
    role: 'QA',
    color: 'violet',
    project: 'factory',
    folder: 'app',
    badges: ['web', 'tree', 'memory:factory'],
  ),
  FakeAgent(
    'coordinator',
    'Mike',
    role: 'Project Manager',
    color: 'green',
    project: 'factory',
    folder: 'app',
    badges: ['web', 'memory:factory'],
  ),
  FakeAgent(
    'researcher',
    'Iris',
    role: 'Researcher',
    color: 'pink',
    project: 'factory',
    folder: 'research',
    parent: 'coordinator',
  ),
  FakeAgent(
    'opus-researcher',
    'Theo',
    role: 'Researcher',
    color: 'blue',
    project: 'factory',
    folder: 'research',
    parent: 'coordinator',
  ),
  FakeAgent(
    'ux-writer',
    'Hana',
    role: 'UX Writer',
    color: 'teal',
    project: 'factory',
    folder: 'app',
    parent: 'coordinator',
  ),
  FakeAgent(
    'builder',
    'Sam',
    role: 'Backend',
    color: 'teal',
    project: 'factory',
    folder: 'app',
    parent: 'coordinator',
  ),
  FakeAgent(
    'backend-2',
    'Lena',
    role: 'Backend',
    color: 'amber',
    project: 'factory',
    folder: 'app',
    parent: 'coordinator',
  ),
  FakeAgent(
    'backend-3',
    'Omar',
    role: 'Backend',
    color: 'green',
    project: 'factory',
    folder: 'app',
    parent: 'coordinator',
  ),
  FakeAgent(
    'frontend',
    'Priya',
    role: 'Frontend',
    color: 'red',
    project: 'factory',
    folder: 'app',
    parent: 'coordinator',
  ),
  FakeAgent(
    'frontend-2',
    'Kenji',
    role: 'Frontend',
    color: 'amber',
    project: 'factory',
    folder: 'app',
    parent: 'coordinator',
  ),
  FakeAgent(
    'frontend-3',
    'Zoe',
    role: 'Frontend',
    color: 'red',
    project: 'factory',
    folder: 'app',
    parent: 'coordinator',
  ),
  FakeAgent(
    'devops',
    'Arjun',
    role: 'DevOps',
    color: 'slate',
    project: 'factory',
    folder: 'app',
    parent: 'coordinator',
    also: ['disk-scout'],
  ),
  FakeAgent(
    'claktor',
    'Felix',
    role: 'Refactoring',
    color: 'slate',
    project: 'factory',
    folder: 'app',
    parent: 'coordinator',
  ),
  FakeAgent(
    'writer',
    'June',
    role: 'Writer',
    color: 'violet',
    project: 'factory',
    folder: 'research',
  ),
  FakeAgent(
    'disk-scout',
    'Disk Scout',
    color: 'pink',
    project: 'disk',
    badges: [],
  ),
];

const claktorySecrets = [
  FakeSecret('s1', 'Cloudflare', ['devops']),
  FakeSecret('s2', 'App Store Connect', ['devops', 'coordinator']),
  FakeSecret('s3', 'OpenAI', ['opus-researcher', 'researcher']),
];

/// Telegram, given to Rivet (who hears it) and Arjun, with its bot token.
const claktoryApps = [
  FakeApp(
    'telegram',
    'Telegram',
    icon: 'send',
    given: {
      'rivet': 'Send you Telegram messages · Read your chat with the bot',
      'devops': 'Send you Telegram messages',
    },
    hears: {'rivet': 'Messages you send the bot'},
  ),
];

/// Mike reports to Rivet, and the rest of the Claktory project to Mike.
const claktoryLeads = {
  'coordinator': 'rivet',
  'researcher': 'coordinator',
  'opus-researcher': 'coordinator',
  'ux-writer': 'coordinator',
  'builder': 'coordinator',
  'backend-2': 'coordinator',
  'backend-3': 'coordinator',
  'frontend': 'coordinator',
  'frontend-2': 'coordinator',
  'frontend-3': 'coordinator',
  'devops': 'coordinator',
  'claktor': 'coordinator',
  'writer': 'coordinator',
  'qa': 'coordinator',
};

/// Matt's Claktory, as it is; with [apps], Telegram installed too; with
/// [leads], [claktoryLeads].
ClaktoryMap claktoryMap({bool apps = false, bool leads = false}) => fakeMap(
  projects: claktoryProjects,
  agents: claktoryAgents,
  secrets: [
    ...claktorySecrets,
    if (apps) const FakeSecret('s4', 'Telegram Bot token', ['app:telegram']),
  ],
  apps: apps ? claktoryApps : const [],
  leads: leads ? claktoryLeads : const {},
);

/// A workspace with [claktoryMap]'s projects and agents, and the map.
Workspace claktoryWorkspace({bool apps = false, bool leads = false}) =>
    Workspace(
      projects: [
        for (final project in claktoryProjects)
          Project(
            id: project.id,
            name: project.name,
            path: '/Users/me/${project.id}',
            folders: [
              for (final folder in project.folders)
                ProjectFolder(id: folder.toLowerCase(), name: folder),
            ],
          ),
      ],
      agents: [
        for (final agent in claktoryAgents)
          Agent(
            id: agent.id,
            name: agent.name,
            projectId: agent.project,
            folderId: agent.folder,
            parentId: agent.parent,
            color: agent.color,
            role: agent.role,
            model: 'anthropic/claude-opus-5-5',
            modelAvailable: true,
            leadId: leads ? claktoryLeads[agent.id] : null,
          ),
      ],
      settings: Settings(userName: 'Matt'),
      map: claktoryMap(apps: apps, leads: leads),
      appsAvailable: apps,
      extensions: claktoryExtensions(),
    )..agents.forEach(_extensionsOn);

/// Some of Claktory's extensions, for the agents' cards.
List<Extension> claktoryExtensions() => [
  Extension(id: 'agents', name: 'Messaging', icon: 'users'),
  Extension(id: 'checklist', name: 'Checklist', icon: 'list-checks'),
  Extension(id: 'memory', name: 'Memory', icon: 'brain'),
  Extension(
    id: 'memory:factory',
    name: 'Claktory manual',
    icon: 'brain',
    base: 'memory',
  ),
  Extension(id: 'tasks', name: 'Tasks', icon: 'terminal'),
  Extension(id: 'web', name: 'Web', icon: 'globe'),
];

/// Mike has Web and the Claktory manual (badges), and Messaging, Checklist
/// and Tasks; Memory is off. Rivet has Messaging, Tasks and Web; the rest
/// Messaging.
void _extensionsOn(Agent agent) {
  final on = switch (agent.id) {
    'coordinator' => {'agents', 'checklist', 'memory:factory', 'tasks', 'web'},
    'rivet' => {'agents', 'tasks', 'web'},
    _ => {'agents'},
  };
  agent.extensions.addAll([
    for (final extension in claktoryExtensions())
      AgentExtension(id: extension.id, enabled: on.contains(extension.id)),
  ]);
}

/// The sidebar's order for [claktoryMap]: as its config lists them.
Map<String, int> claktoryOrder() {
  final map = claktoryMap();
  final ids = [
    'project:factory',
    'folder:factory/app',
    'folder:factory/research',
    'project:disk',
    for (final id in const [
      'rivet',
      'qa',
      'coordinator',
      'researcher',
      'opus-researcher',
      'ux-writer',
      'builder',
      'backend-2',
      'backend-3',
      'frontend',
      'frontend-2',
      'frontend-3',
      'devops',
      'claktor',
      'writer',
      'disk-scout',
    ])
      'agent:$id',
  ];
  assert(ids.every((id) => map.nodes.any((node) => node.id == id)));
  return {for (final (i, id) in ids.indexed) id: i};
}

/// A crowded Claktory: long names, many agents, deep nesting, agents in no
/// project, an admin inside a project, one with messaging off and one no
/// one can reach, grants both ways, and a secret no one has. With [leads],
/// a chain of leads down the site, each project's agents reporting to its
/// first, and those to Rivet.
ClaktoryMap crowdedMap({bool leads = false}) => fakeMap(
  leads: leads
      ? {
          for (var a = 1; a < 14; a++) 's$a': 's${a - 1}',
          's0': 'boss',
          for (var p = 0; p < 5; p++) ...{
            'a$p-0': 'rivet',
            for (var a = 1; a < 3 + p; a++) 'a$p-$a': 'a$p-0',
          },
        }
      : const {},
  projects: [
    const FakeProject(
      'site',
      'A web site with a remarkably long project name that never ends',
      folders: ['Front end of the site', 'Back end', 'Docs'],
    ),
    for (var p = 0; p < 5; p++) FakeProject('p$p', 'Project $p'),
  ],
  agents: [
    const FakeAgent(
      'rivet',
      'Rivet',
      role: 'Admin',
      admin: true,
      reachable: false,
    ),
    const FakeAgent('helper', 'Helper in the workspace', role: 'Odd jobs'),
    const FakeAgent('lonely', 'Lonely', reachable: false),
    const FakeAgent(
      'boss',
      'Boss',
      role: 'Second admin, inside a project',
      project: 'site',
      admin: true,
    ),
    for (var a = 0; a < 14; a++)
      FakeAgent(
        's$a',
        a == 3 ? 'Bartholomew Maximilian Fitzgerald-Worthington' : 'Site $a',
        role: a == 3
            ? 'A role that is longer than any chip could hold'
            : 'Role $a',
        project: 'site',
        folder: a < 4 ? '' : (a < 9 ? 'front end of the site' : 'back end'),
        parent: a == 0 ? '' : 's${a - 1}',
        badges: const ['web', 'tree', 'database', 'memory:factory'],
      ),
    for (var p = 0; p < 5; p++)
      for (var a = 0; a < 3 + p; a++)
        FakeAgent(
          'a$p-$a',
          'Agent $p.$a',
          role: 'Role',
          project: 'p$p',
          also: a == 0 ? ['a${(p + 1) % 5}-0', 's1'] : const [],
          reachable: !(p == 2 && a == 1),
        ),
  ],
  secrets: [
    const FakeSecret(
      'long',
      'A SECRET WITH AN EXTREMELY LONG NAME THAT GOES ON',
      ['s3', 'a1-1'],
    ),
    const FakeSecret('none', 'Nobody has this', []),
    for (var s = 0; s < 8; s++)
      FakeSecret('k$s', 'Key $s', [
        's${s + 2}',
        'a${s % 5}-${s % 3}',
        'helper',
      ]),
  ],
);
