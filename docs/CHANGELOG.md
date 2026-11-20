# Changelog

What each version of Claktory added and fixed, newest first. Claktory shows it in the app, and the Update banner shows what a new version brings.

Every change adds a line under `## Unreleased`: "Added …", "Fixed …", "Changed …", written for the person using Claktory. `scripts/changelog.sh stamp` turns Unreleased into the next version before a release, and `scripts/release.sh` refuses to release without it.

## Unreleased

## v0.1.3
Released 2026-11-20 03:53 UTC.
- Added: See how your Claktory is arranged on the Map: Rivet, each project with its folders and agents, who can message whom and who has which secret. Open it with Map at the bottom of the sidebar. Hover an agent to see what it reaches, and click it to open its conversation.
- Changed: Agents see only the secrets you've given them, and an agent's settings show which ones it has. When an agent needs another, it describes it in its conversation, and you give it one of yours or add a new one. Rivet gives secrets only to agents whose work needs them, and asks you first when an agent asks for more.
- Added: Agents know which device you're writing from, and keep replies short when it's your phone. Each of your messages shows a small mark for its device; hover it, or touch and hold on iPhone, to see the device's name.
- Added: An agent can send one message to several agents at once, and each sees who else got it, so handing out work to a team takes one step.
- Changed: Rivet's prompt stays up to date: each update brings the newest one, unless you've changed it yourself. To go back to the default, click Reset to default in Rivet's settings. Any agent's prompt can also mention you by name.
- Changed: A new agent's conversation says who it is and how to start, instead of being empty.
- Changed: Claktory goes by your Mac's name, such as "Lee's Mac mini", from System Settings → General → Sharing.
- Fixed: After your Mac sleeps or changes network, Claktory reconnects by itself within seconds, and the conversation says so while it does, instead of quietly showing what may be out of date.
- Fixed: Adding your first device no longer disconnects the app on this Mac for half a minute.

## v0.1.2
Released 2026-11-20 02:08 UTC.
- Changed: Settings, the Marketplace and What's new open beside the conversation instead of covering it, so you can read an agent's messages while you change a setting. Drag the line between them to resize, or click × to close.
- Added: Run Claktory on a Linux machine, NixOS included, with the same install command. Then connect your iPhone or Mac with the code it shows.
- Added: An agent that needs a key or password asks you for it in its conversation. Give it one of yours with one click, or add the new one it describes. Requests waiting for you also show at the top of Settings → Secrets.
- Added: Your next message to an agent costs less after a long break. An idle agent with a long conversation summarizes it before picking it up would cost more, and its conversation says "Earlier messages summarized while idle". For Anthropic, set Keep conversations ready to 1 hour in Settings → Providers. Turn it off in Settings → Defaults.
- Changed: Where an agent's earlier messages were summarized, the conversation says "Earlier messages summarized" instead of a token count. Click it to read the summary.
- Changed: Long messages between agents, notes and monitor prompts in a conversation fold to their first line. Click Show more to read them in full.

## v0.1.1
Released 2026-11-20 01:14 UTC.
- Added: Use several Claktories at once, such as this Mac's and a server's. A thin rail at the left of the window shows each one, and when its agents are working or have something new; click one to switch. Click + to add one with its code, and right-click one to rename or remove it. On iPhone, they're a list.
- Changed: Your iPhone and other computers connect to Claktory from any network, private and encrypted, with nothing to set up: no port, address or VPN. In Settings → Devices, click Add a device…, give it a name and scan the code; a code works once, for 10 minutes. Devices you added before need adding again, and other programs on your Mac can no longer control Claktory.
- Added: Give your agents the keys and passwords they need, such as an AWS key or a Cloudflare token, without them ever seeing the values. Add them in Settings → Secrets, then click Give to… to give one to an agent, or ask Rivet to.
- Added: See files beside the conversation. Click a file in the explorer, or let an agent show you one, and it opens next to the conversation: text with line numbers, Markdown formatted, and images you can zoom. Drag a pane by its header to put it beside or below another, and drag the line between two to resize them.
- Added: Use several Claude or ChatGPT subscriptions, and API keys, for one provider. In Settings → Providers, click Add another Claude subscription or Add an API key, and drag them into the order you want. When one reaches its limit or runs out of credit, Claktory switches to the next. The sidebar shows a usage circle for each.
- Added: Claktory for iPhone, a first look. It runs in Xcode's Simulator for now. Connect it by scanning the code your Mac shows in Settings → Devices, then tap an agent to talk to it. It reconnects as soon as you come back to it, and works on its side too.
- Changed: See at a glance what your agents did. Messages, checklist items, notes, monitors, background tasks, files shown, web searches, database queries and secrets Rivet gives or takes back each show as a short line with the details under it. Click a message's recipient to open their conversation, or a search result to open it in your browser.
- Added: Claktory asks your first name the first time it opens, so your agents know who they're working with. Change it in Settings → General.
- Added: Drag the edge of the sidebar or of the panel on the right to resize it, and double-click the edge to reset it. The widths are the same on all your devices.
- Changed: Stop no longer throws away messages waiting for the agent. It stops what the agent is doing, then the agent reads them. /clear still drops them.
- Changed: Agents read your messages sooner. If one is busy with a long command, the command keeps running in the background while the agent reads what you sent.
- Changed: Agents no longer get stuck on a long wait or by copying a huge folder. They're told what to do instead.
- Added: Build files no longer fill your disk. When a project's or agent's build files grow past 25 GB, Claktory clears them while nothing is using them. Change the limit in Settings → Resources → Build caches, or click Clean in the Resources panel to free the space now.
- Changed: Claktory uses less power while your agents work.
- Changed: Your agents know Claktory by its name too: their instructions, the manual and Claktory's messages to them say Claktory. Your folders, files and settings stay where they are.
- Changed: Clearer words across the app: updates, errors, agent settings, extensions, the Marketplace and the Resources panel. Errors offer Ask Rivet to fix.
- Fixed: An agent's settings show Admin as its place as soon as it becomes an admin agent, even while they're open.

## v0.1.0
Released 2026-11-19 22:42 UTC.
- Added: Claktory's source code is public at github.com/claktor/claktory.
- Added: Agents can suggest a message for Rivet. Approve it, with a comment if you like, and Rivet gets it from you, or dismiss it.
- Added: The sidebar and the right panel keep their widths, the same on all your devices.
- Changed: An agent's long command no longer holds it up. After 2 minutes it keeps running in the background, and the agent hears when it finishes.
- Changed: Rivet stays at the top of the sidebar while you focus on a project.
- Changed: Versions are numbered v0.1.0, v0.1.1 and so on, and What's new shows when each came out.
- Fixed: The right-click Copy menu appears small, at the pointer, instead of stretching across the window.
- Fixed: On older installs, macOS could slow Claktory down. The first update fixes it, and Claktory restarts once, about a minute after it starts.

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
