# Changelog

What each version of Factory added and fixed, newest first. Factory shows it in the app, and the Update banner shows what a new version brings.

Every change adds a line under `## Unreleased`: "Added …", "Fixed …", "Changed …", written for the person using Factory. `scripts/changelog.sh stamp` turns Unreleased into the next version before a release, and `scripts/release.sh` refuses to release without it.

## Unreleased

## v0.1.0
Released 2026-11-19 22:42 UTC.
- Changed: an agent's command that runs past 2 minutes no longer blocks it: it moves to the background and keeps running, and the agent hears when it ends, with a note to make it faster. Background commands stop after 30 minutes unless they ask for up to 2 hours, every result says how long the command took, and long output is kept in a file instead of cut off.
- Fixed: the right-click Copy menu stretched across the whole window; it's a compact menu at the pointer again.
- Changed: Rivet stays at the top of the sidebar while a project is focused, so it's always a click away.
- Added: Claktory's source code is public at github.com/claktor/claktory under its license (PolyForm Shield), pushed with every published release after a check that no secret is anywhere in its history.
- Added: the sidebar's and the right panel's widths are kept by Claktory, the same on every device you use it from.
- Added: agents can propose a message to Rivet, which only you can message. The proposal shows in the agent's conversation as a card in Rivet's color; approve it, with a comment, and Rivet gets it as your message, or dismiss it. The agent hears which.
- Changed: versions are numbered like game builds, v0.1.0, v0.1.1, v0.1.2…, instead of by date; What's new shows each one's date.
- Fixed: a Claktory installed before the fix that stops macOS throttling it now gets that fix too, with its first update: about a minute after starting, the backend updates its launch settings and restarts once.

## 2026.11.19-2200
- Fixed: buttons' labels were taken into a text selection (a drag across a page, or ⌘A); now they're left out, as intended.
- Changed: a new app icon in the app's colors: a flat white cog with a hard shadow in Rivet's blue on a graphite tile (Claktory Dev's is the light inverse, labeled DEV).
- Changed: Factory is now called Claktory (Claktory Dev for the dev build): in the menu bar, About, the Dock and Finder, window titles and the app's text. Nothing moves: your data, settings and updates stay where they are.
- Fixed: making an agent's own checkout took minutes in a project with a lot of build output; it now holds only the files git tracks and is ready in moments (the first build in it is a full one).
- Fixed: agents on OpenAI Codex models (ChatGPT sign-in) failed every request since the last update.
- Added: the app shows an agent's role under its name, smaller and fainter, in the sidebar, its header and the Messaging panel, and after its name over its messages and in pickers; set it in agent settings or the New agent form.
- Changed: an agent's own database (the Database extension) is off unless turned on; Rivet turns it on for agents that collect data worth querying later. A database an agent already has stays on disk, for when it's turned on again.
- Added: text you read can be selected everywhere (tool calls, notes, panels, Settings, dialogs, errors), and a right click on it opens a menu with Copy, in the conversation too.
- Added Tell Rivet beside the errors Factory shows (a broken settings file, a failed run, an update that didn't work, a refused request, a failed sign-in and more): it opens Rivet's conversation with the error typed in, ready to send, so Rivet can fix it.
- Added roles: an agent can have a role beside its name ("Coordinator", "Copywriter"), and other agents see it with the name, so an agent named Mike still says what it does. Rivet's role is Admin, and it gives each agent it creates a human first name and a role.

## 2026.11.19-1958
- Changed: agent characters are bigger everywhere they appear (the sidebar, the agent's header, beside messages, in panels and settings), so their eyes read at a glance; agent settings pick only the shape.
- Changed: an agent's character is now just a plain shape (circle, square, triangle, diamond, pentagon or hexagon) in its color, with the same eyes on every agent; faces are gone, and agents that had an organic shape chosen get an automatic one. Rivet is a blue hexagon in a new Factory, and its message box suggests asking for agents that do a job.
- Fixed: on a Mac, macOS no longer throttles Factory's backend and everything agents run (builds, tests, servers), which ran slower and had their timers stretched several times over. Applies when Factory is installed; a Factory installed before keeps the throttling until it's installed again.
- Added a folder browser for a Factory on another machine: in project settings, Choose… now browses that machine's folders, so a folder's path no longer has to be typed.
- Added the Resources panel: the machine's processors, memory, swap and free disk space, the last hour, Factory's processes and the biggest others, and what Factory's data, each agent and each project take on disk. Admin agents such as Rivet are told when the machine runs low, at thresholds you set in Settings → Resources.
- Added: an app connected to a Factory on another machine updates itself from that Factory with the Update banner, as this Mac's app does.
- Added Factory on Linux: the online installer sets up Factory's backend on a Linux machine (x86_64 or aarch64, with systemd) as a service that starts at boot, with remote access on and a pairing code for the app on your Mac.
- Added Settings → Devices: turn on remote access, pair a device with a QR code or a link to copy, see when each device last connected, and unpair it. Click the Factory name at the bottom of the sidebar to connect this app to a Factory on another machine with its pairing code, or back to this Mac's.
- Added the Database extension: every agent has a SQLite database of its own for records it collects or keeps track of, queried with SQL and shown in its Database panel. Admin agents such as Rivet can also read Factory's own records (messages, runs, tasks, checklists, but not agents' memory). On unless turned off.
- Changed: the message box's caret doesn't blink while Factory's window is in the background, so a Factory left open draws nothing while nothing happens.
- Added living agent characters: every agent is a small face in its color, in place of its dot, initial or nothing at all, in the sidebar, the header, Messaging, beside its messages and wherever it's named. Its eyes show what it's doing (thinking, running a tool, news, a failed run, no model, asleep). Pick its shape and face in its settings, or leave them Automatic.
- Changed: a new model, effort, prompt, name, project or folder applies from the agent's next step, even in the middle of a task, and an extension turned on can be used right away, without waiting for the agent's next compaction.
- Fixed: moving an agent in the sidebar no longer changes its color; every agent's color is now saved with it.
- Added the Web extension: agents search the web (DuckDuckGo, then Brave, with no account or key) and read pages as Markdown, PDFs included. On for every agent unless turned off.
- Added the Factory manual's panel: an admin agent's read-only notes from the manual, under the open book icon.
- Changed: each agent's conversation is faintly tinted in its color, so it's always clear whose it is.
- Changed: agents carry only the core of Factory's manual; the rest is the Factory manual, which Rivet opens as it needs it and any agent can be given under Extensions, so every agent's context is about 8,000 tokens lighter.
- Changed: the working wave goes back and forth across the row without resting, so a working agent always shows it.
- Added the Monitors panel: each agent's monitors, when they next fire, and buttons to pause, resume or cancel them.
- Fixed: a second Factory on the same machine (such as a dev one) names its checkouts' branches after itself, so it never takes the branch of an agent with the same id.
- Changed: every agent works in its own checkout (Own checkout) unless it's turned off for it; agents outside a git repository work as before.
- Added What's new: every version's changes on a timeline, from the sidebar's scroll button or the Update banner.
- Added the Monitors extension: agents set prompts that wake them later, at a time, on a schedule, or when a command's result, files or a running command's output change, so they can follow up on their own without polling. On for every agent unless turned off.
- Fixed: the explorer no longer shows a project's files twice when one of its folders is the project's own folder or inside it.

## 2026.11.18-2310
- Added a section for each agent's own checkout in the project's file explorer, with its branch.
- Added rotating examples in Rivet's empty message box; any agent's message box hint can hold several, one per line.
- Added the Own checkout extension: an agent gets its own copy-on-write checkout of its project's git repository, on its own branch and with the build output warm, so agents working in parallel don't step on each other; it lands its work on the main branch when it's ready.
- Added back and forward through agents you followed from messages: a "← name" chip in the header, ⌘[ / ⌘] and the mouse's side buttons.
- Added the Tasks extension: agents see and stop background tasks, and admin agents are told when one runs longer than a day. Its panel lists running and recent tasks with their output, and stops them.
- Added a live clock on tool calls (how long they've run, and when a command will time out) and how long each took. Commands time out after 10 minutes unless they ask for longer (up to an hour).
- Added installing Factory on another Mac with one command, after which that Factory downloads new versions itself and offers them with the Update banner.
- Added the subscription's plan ("Claude Max 20x", "ChatGPT Pro") to the usage hover.
- Added a row for each tool call that says what it does (the command, the file, the recipient), even while the model is still writing it.
- Added highlights: Factory makes new agents, projects, folders and changed checklist items glow briefly, and agents can point things out in the app. Turn them off in Settings.
- Added project folders: a project can have several folders, each agent works in one of them, and the sidebar and the explorer show each folder.
- Added a wave of the agent's color across its sidebar row while it works.
- Added the sidebar's "+", which asks Rivet to set up projects and agents.
- Changed: each agent keeps the panel it has open.
- Changed: when you change an agent's own settings, it's told before its next step, and its conversation's cache is kept.
- Fixed: Factory no longer accepts connections from web pages.
- Fixed: right after signing in with a subscription, the usage hover says usage hasn't been read yet instead of "No usage limits to show".

## 2026.11.18-2123
- Added a hint in each agent's message box, set in its settings.
- Added clicking an agent's message bubble to open that agent.
- Added the checklist's blocked status, with a note saying what an item waits on, and a reminder for agents that stop with work outstanding.
- Added the map of who can message whom, for admin agents.
- Added unsent drafts, kept per agent on this device.

## 2026.11.18-2054
- Added the Memory extension: an agent's own preferences and notes, kept across compactions.
- Added the Messaging panel: whom an agent can message, and who can message it.
- Changed: the Checklist panel shows its history by day, with outstanding work at the bottom.
- Added slash commands: /clear and /compact.
- Added queued messages, shown faded until the agent takes them.
- Added grouped tool calls: the calls one reply made share a block.
- Added the providers' logos to the usage meters.
- Changed: an agent's system prompt and tools stay the same for a whole context, so providers' caches hold.
- Fixed: clicking an agent no longer drags it out of the agent it's nested under.

## 2026.11.18-1948
- First release: persistent agents in projects, the Messaging and Checklist extensions, subscription usage meters, the unseen marker, arranging and nesting agents in the sidebar, updates with rollback, and Rivet.
