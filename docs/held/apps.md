# Held until Apps ship

Apps work in the dev Claktory but aren't offered in the person's until Telegram has worked end to end (`SHIPPED` in `crates/factoryd/src/apps/mod.rs`). What the manual and the changelog say about them waits here; when Apps ship, each part goes back where its heading says.

## docs/MANUAL.md: the core's "## Apps", right before "## Secrets"

```markdown
## Apps

- An **app** is something Claktory runs once for all its agents, such as Telegram (message your agents from your phone). {{User}} installs apps and gives them to agents in Settings → Apps. An app runs walled off, reaches only the sites {{user}} approved, and keeps its own secrets in the vault, where no agent gets them.
- An agent given an app has the tools of what it was given (`telegram_send`, say), and its system prompt has a section on the app. Given one mid-context, it's told in an event from Claktory and calls the tools through `extension_call`; taken back, they stop working.
- An agent {{user}} chose to hear an app gets its events (a message to the Telegram bot, say) as `<event source="app" from="Telegram">`; several close together come as one, a line each.
```

## docs/MANUAL.md: Settings (the app-settings note), after "**Secrets:**"

```markdown
- **Apps:** the apps installed (such as Telegram), each with its state (Running, Off, Needs setup, Paused because it kept quitting, Can't run on this computer, with why), a switch that runs it or turns it off, its own line (how to finish linking it, say) and what its setup still needs, each secret with **Set…** (kept in the vault for that app alone; no agent sees it). Opened: what it's for, its setup (**Replace…** changes a secret), the agents it's given to with what each may do and which of its events each hears (**Change…**: a switch per scope and per event, an event only with its scope; **Take back**), **Give to an agent…** (every scope on to start with), the sites it can reach, its version, publisher and how it's vouched for, and **Uninstall…** (**Keep its data** keeps it for a later install). **Install from a file…** picks a `.clapp` on this device and shows what it would reach before **Install**: the sites, what agents given it can do, the events it sends, the secrets its setup asks for, its memory and storage, and its checksum, with a warning when Claktory hasn't reviewed it; installing repeats the checksum, so what installs is the file reviewed. A secret given to an app shows under Secrets as given to the app.
```

## docs/MANUAL.md: Map (the app-map note), after "Secrets:"

```markdown
- Apps: each installed app (such as Telegram), with its icon, in **Apps** along the bottom beside the vault, linked to the agents it's given to, with an arrow into each agent that hears its events; a secret given to an app links to the app. An agent's card lists its apps, with what it may do with each and the events it hears.
```

The app-map note's summary goes back to "who has which secret and app":

```markdown
<!-- admin: app-map | The Map: how Claktory is arranged, who can message whom, who has which secret and app, what each agent reads and writes, and its powerful extensions. -->
```

## docs/CHANGELOG.md: under "## Unreleased"

```markdown
- Added: Apps connect Claktory to other services, starting with Telegram: install one from a file in Settings → Apps, give it the key it needs, and choose which agents can use it and which hear from it. Each app runs walled off and reaches only the sites you approve, and the Map shows which agents have it.
```
