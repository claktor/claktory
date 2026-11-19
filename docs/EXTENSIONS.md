# Making an extension

An extension is a capability an agent has on top of its coding tools: messaging other agents, keeping a checklist, remembering what the person prefers. Each one is on or off per agent, with its own options, and adds a section to the agent's system prompt, tools, data, and possibly a panel in the app.

Extensions are built into factoryd for now, in `crates/factoryd/src/extensions/`. These are the examples throughout:
- **Messaging** (`agents.rs`, id `agents`, on by default): agents message each other and see who's working. It was called Agents at first; the id, config keys and tool names kept that name, since ids are forever and only the display name changed.
- **Checklist** (`checklist.rs`, off by default): the agent keeps everything the person asks for and works through it over days.
- **Memory** (`memory.rs`, off by default): the agent's own preferences (what the person wants, each with why) and knowledge notes, kept across compactions. Never shared with other agents.
- **Highlights** (`highlight.rs`, off by default): the agent makes parts of the app glow to show the person what it means. No data and no panel: highlights are only sent.
- **Tasks** (`tasks.rs`, on by default): the agent sees and stops background tasks, its own or, with `see_all` (or as an admin), every agent's. Its data is the `tasks` table the runner already keeps; it also runs a monitor that tells admin agents of tasks running past `task_alert_hours`.
- **Monitors** (`monitors.rs` with `monitors/when.rs` and `monitors/run.rs`, on by default): the agent sets prompts that wake it later, on a time, a schedule, a command's result, files or a running command's output. Its data is the `monitors` and `monitor_events` tables; a scheduler started with the Factory (`monitors::monitor`) fires them as events with `EventSource::Monitor`, and the nudge asks it (`monitors::will_wake`) whether a monitor will wake the agent anyway.
- **Resources** (`resources.rs` with `resources/sample.rs` and `resources/disk.rs`, id `resources`, on for admin agents by default via `admin_default`): how the machine and Factory use processors, memory and disk. No tables: what it knows lives in `Factory::resources` and is lost on restart. A sampler started with the Factory (`resources::monitor`) reads the machine with `sysinfo` every minute, keeps an hour, and tells Factory's processes apart by process group (`getpgid`), matched to the `tasks` table's pgids and monitors' watches; free space comes from `statfs`/`statvfs`, and a separate loop walks Factory's data and projects for their sizes every 20 minutes, on a time budget (`disk.rs`). When memory, a volume or the processors cross `Config::resources`' lines it delivers one Factory event per crossing to every agent with it on, re-armed once the machine recovers. Its panel is `v1::Resources`; the thresholds are in `Settings`.
- **Own checkout** (`tree.rs`, id `tree`, on by default; agents outside a git repository get no checkout and no tool, only a line saying why): the agent works in its own copy-on-write checkout of its project's repository (`crate::trees`), on its branch `factory/<agent>` (`<home's name>/<agent>` for a Factory whose home isn't ~/.factory), and lands its work with `tree_land`. It hooks into the host rather than adding data: the runner makes the checkout before a run (`Factory::ensure_checkout`) and `runner::places` points the agent's folder there, making the main checkout read only; config changes retire checkouts that can go without losing work (`Factory::retire_checkouts`).
- **Database** (`database.rs` with `database/sql.rs`, id `database`, off by default): the agent's own SQLite database, `<home>/agents/<id>/database.sqlite`, made on its first write, with `db_query`, `db_exec` and `db_schema` (it overrides `owns_tool`, since its tools are `db_*`). Its instance `database:factory` reads factory.db for admin agents, opened read only and without the `memory_*` tables. `database::access` alone decides which database a call means and whether the agent may use it, and every statement runs through `sql::open`, whose SQLite authorizer is the one place reads and writes are allowed or refused, table by table: sharing a database between agents hooks in there. Its panel lists the tables (`v1::Database`).
- **Web** (`web/`, id `web`, on by default): the agent searches the web (`web/search.rs`, with each engine's request and parser in `web/engines.rs`) and reads pages as Markdown (`web/fetch.rs`, `web/convert.rs`). No data and no panel. What every agent shares, since they all search from one IP (pacing, cooldowns, caches, HTTP clients), lives in `Factory::web`. All its HTTP goes through `primp`, one pinned browser profile per engine; the `#[ignore]` tests `web_live_*` check the fingerprints and engines by hand.

Loadable third-party extensions come later. Everything below is shaped so they can fit the same interface: a manifest that says what the extension is, contributions the runner collects, data read through one function, and views the app renders. Don't special-case an extension anywhere outside its own module.

## The parts

### Manifest

`Manifest` in `extensions/mod.rs` says everything Factory needs to show an extension without running it:

| Field | Agents | Checklist | Memory |
|---|---|---|---|
| `id`: lowercase, stable forever (config keys, tool names and tables use it) | `agents` | `checklist` | `memory` |
| `name` | Messaging | Checklist | Memory |
| `icon`: a [Lucide](https://lucide.dev/icons) name, shown in the extension bar | `users` | `list-checks` | `brain` |
| `description`: one sentence, shown in the Marketplace | Message other agents… | Everything the person asks for… | The agent's own memory… |
| `default_enabled`: whether agents have it unless it's turned off | true | false | false |
| `options` | `reachable`, `admin`, `see_map`, `also` | `nudge` | none |

Make an extension default-on only if every agent is better off with it, as with Agents. Anything situational (Checklist, memory) is off by default; the person or the admin agent (Rivet) turns it on where it's wanted. One meant for admin agents only, such as Resources, stays off by default and returns true from `Extension::admin_default`: admin agents then have it unless it's turned off.

Add the extension to `ALL` in `extensions/mod.rs`, kept sorted by id. That order is the order of prompt sections and tools, so it has to be stable (see "Prompt sections").

### Options

Each option is an `OptionSpec`: `id`, `label` (what the app shows, e.g. "Other agents can message it"), `description`, and a `kind`:
- `Switch`: on or off, with `default_on`.
- `Agents`: a list of agent ids. With `includes_project`, the agent's own project's agents always count, and the app shows them checked and greyed.

The app renders options generically from the manifest, so a new option needs no app change. Read an option's effective value with `extensions::switch(agent, id, option)` or `extensions::agent_list(...)`; never read `agent.extensions` directly.

In config.json, an agent's settings live under its `extensions`, by extension id:

```json
"extensions": { "agents": { "reachable": false, "admin": true, "also": ["researcher"] }, "checklist": { "enabled": true } }
```

Every key is optional and takes the default when absent. The app writes only values that differ from the default. Settings for an unknown extension or option are kept and logged, not an error, so removing an extension never breaks anyone's settings. A value of the wrong kind is an error, so a typo doesn't silently do nothing. When an agent is deleted, `forget_agent` removes it from every agent list.

### Instances

An extension can declare instances (`fn instances`, `InstanceSpec`): named second uses of it, each on or off per agent on its own, such as the Factory manual, `memory:factory`, Memory serving Factory's own manual read only. An instance's key, `<extension id>:<name>`, stands for it wherever an extension id does: config.json (`"memory:factory": {"enabled": true}`), `extensions::enabled`, the catalog and agents' extension lists (each instance right after its extension, with `base` set), `ExtensionChange`, panels and settings notes. Instances have no options; their default is `InstanceDefault` (Admin: on for admin agents unless turned off).

The extension serves its instances itself, with the same tools: the runner includes it when it or any of its instances is on (`extensions::active`), and `prompt`, `tools` and `instance_view` look at which the agent has on. A tool several of the agent's instances can serve takes an optional `instance` parameter (left out: the agent's own), and only then, so its schema costs nothing otherwise. An instance's pinned section is `PromptKind::InstancePinned(key)`.

### Prompt sections

`prompt(factory, inner, agent)` returns what the agent's system prompt says about the extension: when to use it and how to use it cheaply. Keep it short. Each section has a `PromptKind`.

An agent's whole system prompt and tool list are written when its context starts and sent byte for byte until the next compaction (the segment's frame, `transcript::Frame`), so providers' prompt caches hold. Only the person changing the agent itself writes them sooner, from its next model call (`runner::settings_now`): its model, instructions, name, project or folder. So whatever a section says stays as it was for the rest of the context: never put live data in it, and never count on it changing. What the agent needs to learn meanwhile reaches it appended to the conversation: tool results, or a Factory event before its next step (as Messaging's roster changes do, in `runner::changes`, and changes to the agent's own settings, in `runner::settings_note`). Turning an extension on or off, or changing its options, is such a change: the agent is told at once, its tools and section come or go with its next context, and a tool of an extension turned off is refused meanwhile (`runner::refused`). An extension turned on can be used at once all the same: the note carries its section and its tools' names, descriptions and parameters (`runner::extension_news`), and the agent calls them through `extension_call {tool, args}`, a tool every frame has, which runs any tool of the extensions on for it now. So a tool's description and parameters must make sense on their own, outside the tool list.

**Static** sections have no counts, statuses, items or dates: they're the same text for every agent with the extension, at any time. Live data goes through tools.
- Messaging's section is fixed text plus a one-line roster of who the agent can message, as it was when the context began. When the roster changes, a Factory event before the agent's next step says who it can now message and who it can't any more.
- Checklist's section is fixed text: when to add items, and how to use statuses (in_progress only while someone is on it right now; waiting work stays open with its owner, handed out one or two items at a time).
- An extension can act on its own, not only through tools: Checklist's nudge (`extensions/nudge.rs`) watches for an idle agent with outstanding items that nothing will wake, and appends one Factory event to its conversation. Anything like it must append (never touch the prompt), say what's due tersely, and never repeat itself until something changed, so it can't loop.

**Segment-pinned** sections (`PromptKind::SegmentPinned`, at most one per extension) may hold data. One is written when a segment starts (the agent's first run with the extension on, and right after each compaction, even mid-run), saved in that segment's file under `pinned`, and reused unchanged on every run until the next compaction, restarts included. So it never breaks the cache mid-conversation. What changes during the segment reaches the agent through its tool results, and the pinned text should say so. Memory's "Your memory" section is pinned: the preferences in full and the notes' index. Pinned sections go after the static ones in the system prompt.

### Tools

`tools(factory, agent)` returns the extension's tools. Build them with `extensions::tool(...)`.
- **Names** are `<extension id>_<verb>`: `agents_send`, `agents_status`, `agents_map`, `checklist_add`, `checklist_update`, `checklist_list`, `checklist_item`, `memory_update`, `memory_read`. They're public names, so don't rename them.
- **Schemas aren't enforced while sampling** (no strict tools). Providers cap how many optional parameters strict tools may have in all (Anthropic: 16), and extensions together would exceed it. So optional parameters are plain properties left out of `required`, never `anyOf` with null. Validate arguments in the handler, and name the valid values in the error.
- An extension may give a tool only to agents with an option on (Messaging's `agents_map` needs `see_map` or `admin`): `tools` reads the agent's settings, and since the tool list is part of the frame, the tool arrives with the agent's next context.
- **Handlers** hold a `Weak<Factory>`, run on the agent's behalf (the agent id is the actor), and return text.

Write tools for few tokens:
1. **Few tools, each with a clear job.** Batch instead of adding verbs: `checklist_add` takes many items, `checklist_update` takes a list of changes, and `memory_update` takes a list of ops, applied all or none.
2. **Short descriptions.** One or two sentences, with the parameters' meanings and valid values. Static, like prompt sections.
3. **Filters and limits with sensible defaults; never a full dump.** `checklist_list` shows open items, at most 50, oldest first, filtered by status, day, owner or text. When it cuts off, it says how to narrow: `+34 more; narrow with q, day, owner or status, or raise limit.`
4. **One terse line per record, with short ids.** `#12 [~] Fix login (frontend) 11-18`, `frontend Frontend: working 12m, running bash; 1 queued`. Use small per-scope numbers (#1, #2), not UUIDs. Leave out what the agent can't use.
5. **Writes return the change, not the state.** `Added #1-#3.`, `#1 done, noted; #2 owner frontend.` Never echo the list.
6. **History only on request.** `checklist_item` shows one item's log; nothing else does.
7. **Errors that teach.** Say what's valid or what to call next: `No agent you can message is called "x"; you can message: writer, scout (Lab).`
8. **Ids that never need guessing.** Names repeat across projects, so output gives ids, plus the project wherever it isn't the caller's own. Look agents up with `agents::find`: by id, then name, preferring the caller's project, and failing with the choices when it's still ambiguous.

### Data

An extension's data lives in factory.db, in tables named after it (`checklist_items`, `checklist_events`), added as a migration in `store.rs` (`MIGRATIONS`). Never change or reuse an existing migration; add a new one.
- **State** is the current record: `checklist_items` (agent, id, text, status, owner, created, updated).
- **Events** are append-only and timestamped, written in the same transaction as each change: `checklist_events` (agent, item, time, actor, kind, value). The actor is an agent id, or '' for the person. Record events from day one. They're what makes history questions answerable later, such as how many items were done per hour, which current state alone can't tell.
- Every record carries the `agent` it belongs to.
- Data that's always loaded (a pinned section) needs a **cap**, so it can't grow without bound. Memory caps preferences at 16,000 characters in all and notes at 100. A write over the cap fails with the current entries and their sizes, so the agent can merge or remove some in the same call.

Everything reads the data through **one query function** per extension (`checklist::query`): the extension's own tools, its panel, and other agents reading through `agent`. A later extension that charts checklist events reads through it too, never through the tables. That function is where permission checks will go.

An agent deleted in the app takes its extension data with it (`delete_agent_data`).

### Its panel

An extension can show a panel beside the selected agent. Its icon appears in the extension bar for agents that have it on; clicking it opens the panel.
- `view(factory, inner, agent)` returns the panel's content as an `ExtensionView`: `extension_id`, `agent_id`, `title`, and a `body` oneof in the protocol. Checklist's body is `Checklist { items }`; Memory's is `Memory { preferences, notes, changes }`; Messaging's is `Messaging { reaches, reached_by }`, ids only, since the app has names, colors and status from the workspace. A new panel adds its own case to the oneof (proto rules: only add, never renumber) and its widget to the app.
- Which panel is open is backend state (`Workspace.open_panel`, set with `OpenPanel`, kept in factory.db), the same on every device.
- Factory sends the view when it changes: in the connect snapshot, when the panel opens, when the selected agent changes, and whenever the settings change (so a view computed from config, like Messaging's, stays current without more code). After writing data, call `factory.refresh_view(agent)`, which sends it only if that agent is selected and the view changed.
- Anything the person can do in a panel must also be a tool, recording who did it (the person is actor ''). The Checklist and Memory panels are read-only today.

## Adding one

1. Create `extensions/<id>.rs` with a `Manifest` and a type implementing `Extension`, and add it to `ALL` in id order.
2. Write its prompt sections (static, plus a pinned one only if it must show data) and its tools, following the token rules above.
3. If it keeps data: add a migration with its state and event tables, one query function, and its deletion in `delete_agent_data`.
4. If it has a panel: add its body to `ExtensionView` in `proto/factory/v1/factory.proto`, run `scripts/gen-proto.sh`, implement `view`, call `refresh_view` after writes, and agree on the panel with Frontend.
5. Tests: its tools through the scripted model (`call <tool> <json>`; see `crates/factoryd/tests/extensions.rs`), its settings, and its view.
6. Describe it in `docs/MANUAL.md` (Extensions, and its config keys under "Changing Factory as an agent"), so every agent knows it.
