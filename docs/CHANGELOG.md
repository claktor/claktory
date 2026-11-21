# Changelog

What each version of Claktory added and fixed, newest first. Claktory shows it in the app, and the Update banner shows what a new version brings.

Every change adds a line under `## Unreleased`: "Added …", "Fixed …", "Changed …", written for the person using Claktory. `scripts/changelog.sh stamp` turns Unreleased into the next version before a release, and `scripts/release.sh` refuses to release without it.

## Unreleased

## v0.1.14
Released 2026-11-21 18:44 UTC.
- Added: Move Claktory to another computer, such as a new Mac or a Linux server, with everything in it: agents, conversations, secrets, sign-ins and project folders. In Settings → General, choose Move to another computer and follow the steps: your agents keep working while it copies, and your devices follow it by themselves. From a terminal, factoryd receive and factoryd move do the same, and factoryd export --final with factoryd import moves it by file. On a computer with no Claktory yet, the install command with --receive (or --import for a move file) installs it and brings the move in, in one line.
- Added: Chats, for a quick question that doesn't need an agent: click New chat in the sidebar (on iPhone, in the menu). A chat can search the web and read pages, and can suggest that Rivet set up an agent or project for you; approve or dismiss its suggestion in the chat or from Needs you. Your agents never see your chats.
- Added: Needs you, at the top of the sidebar, gathers everything that waits for your answer, from every agent and pipe, in one place. Open an item to answer it right there, without opening the conversation. Agents can also ask you a question when they need your decision, and carry on once you answer on its card. On iPhone, tap See all under Needs you in the menu. A small chip at the top of every conversation shows how many things need you; click or tap it to answer them right there.
- Added: Send voice messages from the Mac or iPhone: on the Mac, click the microphone in the message box and speak; on iPhone, hold it while you speak and let go to send, or slide left to cancel. The computer Claktory runs on turns your words into text, so the recording goes nowhere else, and ▶ plays it back. The first time, Claktory asks before it downloads its speech model (740 MB).
- Added: Let your agents talk with someone else's: a pipe connects one of your projects to a project in another person's Claktory. Choose Connect a pipe… in the project's settings. You both approve by comparing digits, and you choose which of your agents may use it. Your secrets never go through a pipe, you review what your agents send unless you turn that off, and an agent that has read a pipe asks you each time before it uses a secret.
- Added: Connect a new device to your Claktories without a code: sign in on it with your email, pick a Claktory from the list, and allow it on a device you already use. Devices you added with a code are asked to sign in too, within a week, or they stop connecting until they do.
- Changed: When an agent needs a secret, you only approve it: Rivet gives the agent one of your secrets that fits, or asks you to add one in its conversation, filled in and ready to save. You can still choose or add it yourself on the card, and Needs you counts it only while it waits on you.
- Added: Reply to any message: right-click it on a Mac, or touch and hold it on iPhone, and choose Reply to quote it in your message.
- Changed: An agent's replies show as bubbles in its color, so you can tell them from other agents' messages at a glance.
- Added: When a model declines a step for safety reasons, it shows as a card with the model's reason, and Claktory can try it again on a fallback model: choose one under Fallback model in an agent's settings, or for every agent in Settings → Defaults. The conversation says which model answered. Claude models with their own fallback may switch by themselves, and the conversation says that too.
- Added: Agents can look at images. An agent whose model reads images opens a PNG, JPEG, GIF or WebP file and sees it as a picture; one too big is scaled down for it automatically.
- Changed: On iPhone, the conversation fills the screen. Tap Menu, or swipe in from the left, for your Claktories, a search, what needs you, and all your projects, agents and pipes. Settings, Files and other pages open over the conversation, and Back or a swipe from the left takes you back.
- Changed: The iPhone app feels more at home: buttons and fields are sized for your finger, text selects with the iPhone's own handles and menu, dialogs rise from the bottom as sheets you can drag down to close, and conversations have larger text and a message box laid out like Messages. Small gray text, such as times and who a comment goes to, stands out more so it's easier to read, and queued messages are dimmed less. Tap or scroll the conversation to put the keyboard away.
- Changed: On iPhone, Settings is a list of sections, each showing what it's set to. Tap one to open it. Forms rise as sheets with Cancel and Save at the top, each field brings the keyboard it needs, and everything fits at the largest text sizes. Signing in and the first-launch steps fill the screen, and the iPhone offers the emailed code right above the keyboard.
- Changed: On iPhone, files open full screen with Back and More, and you comment the iPhone way: select text and tap Comment or Suggest, then write in a sheet above the keyboard. Tap a comment's dot to read it and reply. On an image, tap Comment, then tap a spot, drag across an area or tap Whole image; you can pinch to zoom first. On a video, tap Comment twice to comment on the moment on screen.
- Changed: On iPhone, Usage now opens with your subscriptions: one row per account shows how much of each limit you've used, and a tap shows when each resets and lets you use a usage reset. The ranges sit side by side above the charts.
- Changed: On iPhone, the Map fits the phone: tap an agent for its card, Lines to show or hide each kind, and Fit to see it all again. Search lists everything on it, with who reports to whom, so you can find an agent quickly or read the Map with VoiceOver. Usage, the Marketplace and What's new open as pages with Back.
- Changed: On iPhone, extension panels open in a sheet you can drag down to close, with controls big enough to tap and text that grows with your text size. The ⋯ list puts the extensions with something to show first, and Stop and Cancel ask before they end a task or a monitor.
- Changed: Every Claktory now asks you to sign in with your email, including ones you've used before and ones where you chose Not now. If claktory.com can't be reached, choose Continue for now and sign in once it's back. The app behind the sign-in stays dimmed until you're signed in, on iPhone too.
- Added: On a Mac, when you scroll up in a conversation, a button at its bottom right takes you back to the latest message and says how many replies came in meanwhile.
- Added: Choose your avatar's colors yourself: Settings → Profile shows all twelve to pick from, and Shuffle colors still picks some at random.
- Added: Delete your Claktory account from the app: Settings → Account → Delete account… opens the step that deletes it at claktory.com.
- Fixed: While the app was connected to a Claktory on another computer, other apps and programs on your Mac or iPhone could use that connection too. Now only the Claktory app can.
- Fixed: Agents that can read Claktory's records, such as Rivet, could see comments you hadn't sent yet. Now an agent sees a comment only once you send it.
- Changed: Agents' file tools no longer reach into Claktory's own data. An agent can read and write its own folder there (its uploads and task logs), read what pipes brought it, and nothing else: your sign-ins, settings and other agents' folders stay out of reach. Agents that manage Claktory for you can still edit its settings file.
- Changed: Claktory installs an update only if it's signed by Claktory's own release key, so an update can't come from anyone else, even if the place it's downloaded from were taken over.
- Fixed: Reconnecting to a Claktory on another computer is much quicker: the apps reconnect as soon as your network changes (from Wi-Fi to cellular, say) or your Mac wakes, a try that can't get through gives up after 8 seconds instead of 20, and nothing waits on claktory.com. Can't connect no longer flashes while a try is under way; you see Reconnecting… until one has actually failed.
- Fixed: When an iPhone or Mac couldn't read the connections and sign-in it keeps (on an iPhone not yet unlocked after a restart, say), Claktory looked as if it had never connected and could overwrite them. Now it says so and offers Try again or Start over.
- Changed: When you switch Claktory to your own relays, the devices you've added keep connecting without being added again. For that, Claktory lists how to reach it in a public directory: its address, never your data.
- Fixed: A Claude limit that covers only some models paused agents on the others too, so Fable's own weekly limit stopped Opus, Sonnet and Haiku. Now each limit pauses only the models it covers; the weekly limit for every model still pauses them all.
- Fixed: After you used a Claude usage reset, Claktory could say none were left while one remained. It now shows Anthropic's own count.
- Fixed: After you opened your secrets with your recovery key, they could lock again the next time Claktory started and ask for the key once more. Now they open by themselves again from then on.
- Fixed: After a security change at claktory.com, Claktory kept saying it couldn't reach claktory.com. It now asks you to sign in again, and your secrets unlock by themselves once you do.
- Fixed: When the secret you chose for an agent's request didn't set the name it asked for, the card said so in a way that was hard to follow. It now says plainly what the secret doesn't set and asks you to check it's the right one. Its tabs now read Your secrets and New secret.
- Fixed: Text copied from a conversation lost its shape, so a list or an email ran together when you pasted it. It now keeps paragraphs apart, each list item on its own line with its bullet or number, and nested items indented.
- Fixed: Dragging files from the Finder onto the Mac app works again, even after you switch agents. The whole window dims and says which agent they'll go to, or why they can't go there, such as in a chat.
- Fixed: VoiceOver stopped saying whose message box you were in once you started typing, and didn't say when something was a button or when it was dimmed. It now says all three.
- Fixed: Opening the app or switching to another Claktory briefly showed screens meant for a new one, such as asking your name or No agents yet. It now says Connecting… until your Claktory has loaded.
- Fixed: A conversation no longer jumps far back to an old card waiting for your answer when you open it, and no longer flashes or loses its place when you click its agent again. Following new messages at the bottom stays smooth after going back to a card. To reach an older card, click its orange badge.
- Fixed: Signing in with a different email after signing out took two codes: the first was turned away. One code now does it.
- Fixed: The Usage page's Today, 7 days, 30 days and All time stacked on top of each other. They sit side by side again.
- Fixed: On iPhone, scrolling the list of agents no longer starts rearranging them; hold an agent to move it.
- Changed: When the iPhone app asks for the camera or your local network, it says it's for connecting to your Claktory, wherever it runs, not only on a Mac.
- Fixed: Allowing a new device from a terminal with the digits it shows (factoryd allow DEVICE --number DIGITS) works again; it took the digits for part of the device's name.

## v0.1.13
Released 2026-11-20 20:40 UTC.
- Changed: Only your own user account can connect to Claktory on the computer it runs on. Other people's accounts there can't.
- Changed: Claktory asks you to sign in with your email when you first open it, so your secrets unlock by themselves after a restart. If you've used Claktory before, choose Not now to sign in later in Settings → Account.
- Added: Setting up a new Claktory now walks you through connecting a model: sign in with Claude or ChatGPT, or use an API key. Then Rivet greets you, ready to help.
- Added: Set up Claktory on a server from its terminal, or let an agent there do it for you: sign in with the emailed code, give your name, add a device and an API key, all without the app. The installer can run without questions and makes the factoryd command ready to use.
- Added: Use the Mac app with a Claktory on a server, without setting one up on your Mac: the first launch asks where your Claktory runs, and a link from your agent connects it. The server has no screen to allow your Mac on, so the app shows the command to run there, ready to copy, and your Mac shows there under its own name.
- Added: Ask Rivet to add a device, such as your iPhone. Its code shows in Rivet's conversation for you to scan or copy, and you still allow the device yourself. Rivet can also list your devices and remove one when you ask.
- Added: Rivet can open a screen for you, such as Settings → Devices or the Map, in the app you're using, so you don't have to look for it. Your other devices stay as they are.
- Changed: Settings → Providers lists only the providers you've signed in to. To add one, type its name in Add a provider… and pick it from the list. Sign in… on an agent opens Settings right at Providers.
- Changed: The Mac download's window shows an arrow from Claktory to Applications, so it's clear where to drag it.
- Fixed: A mistyped or expired API key looked connected until Rivet's first answer failed. Claktory now checks a key with its provider first and tells you right away.
- Fixed: When a provider refused an agent's key or sign-in, its conversation showed the provider's raw error. It now says so plainly, with Change key… or Sign in again….
- Fixed: When another Claktory on the same computer (another user's, say) already used the port Claktory needs, Claktory and its installer only said it didn't start. Now they say why.
- Fixed: Updating Claktory restarted your agents twice, the second time about half a minute after the first. Now it restarts them once.

## v0.1.12
Released 2026-11-20 19:12 UTC.
- Added: Remove Claktory from this Mac in Settings → This Mac. Keep your data to pick up where you left off later, or delete it too.
- Added: When the app and your Claktory are too far apart in version to work together, the app says which one to update, and how.
- Changed: An agent's card on the Map shows who it reports to and who it leads.
- Changed: Click a switch's label or note to turn it on or off, not only the switch itself.
- Fixed: When one of your Claktories couldn't go on (stuck setting up, signing in or needing an update), it could fill the window with no way to switch to another. Your other Claktories now stay in reach, on the Mac and on iPhone.

## v0.1.11
Released 2026-11-20 18:23 UTC.
- Added: Download Claktory for Mac from claktory.com and open it: it sets itself up, and offers to move itself to Applications.
- Changed: Comment on files as in a shared document: select text, click Comment right there, and press Return to send it to the agent at once. Comments show beside the text only when there are some, as small dots on a narrow pane. On an image or a video, click Comment, then click a spot or drag across an area; numbered pins mark each comment.

## v0.1.10
Released 2026-11-20 16:55 UTC.
- Added: When an agent is waiting for your answer, such as a secret it asked for, its row in the sidebar shows an orange badge with how many. Click the agent to go straight to the oldest one.
- Added: An agent can lead others: it sees how the agents under it are set up, so you don't have to explain. Choose an agent's lead in its settings.
- Added: An avatar for you, your initials on a color, beside your messages and comments on every device. Shuffle its colors in Settings → Profile.
- Changed: When all of an agent's accounts reach their usage limits, it now waits instead of failing, and carries on by itself when the first one resets. Its row in the sidebar says what it's waiting for, and messages sent meanwhile wait with it. Adding an account or using a usage reset gets it going sooner.
- Changed: A secret now says exactly what it can do, and the Map shows it. If a secret's names differ from the ones an agent asked for, you can give it under the agent's names too. Agents can also change or withdraw a request they've made.
- Changed: Messages from other agents read like any other: clicking one selects text and follows links. To open that agent's conversation, click its name above the message.
- Changed: Dragging files over a conversation now shows where they'll go: it dims, takes the agent's color and says who gets them.
- Fixed: After your computer restarted, agents' first tries could fail, and some waited forever for background tasks the restart had ended. Claktory now waits for the network first, and tells agents their tasks ended.
- Fixed: An agent's request for a secret could expire while the agent kept working. It now waits 12 hours for you, however busy the agent is.
- Fixed: Agents can't be fooled by a message or a task's output that pretends to be from you or another agent.
- Fixed: Messages agents send each other, and their Talk to suggestions, show quotes, ampersands and angle brackets as they are, instead of codes like &quot;.

## v0.1.9
Released 2026-11-20 15:04 UTC.
- Added: Give agents their own Cloudflare access, with only what their work needs. Connect Cloudflare in Settings → Secrets, or when an agent suggests it: make a token on the page Claktory opens, then paste it back. No agent gets anything until you or Rivet give it a secret made from the connection.
- Added: See where your tokens go: the Usage button at the bottom of the sidebar shows how many each agent, provider, model and account used today, over 7 or 30 days, or all time, with a chart and your subscription limits.
- Added: Use your Claude or ChatGPT usage resets right from Claktory: hover a provider's circle at the bottom of the sidebar to see how many you have left and use one. A dot on the circle tells you when you're close to a limit a reset can refill.
- Added: Make the whole app bigger or smaller, as in a browser: press ⌘+ or ⌘−, and ⌘0 for actual size, or set Zoom in Settings → Display. Each device keeps its own.
- Added: An agent can suggest you talk to another agent that's better placed to help. Its suggestion's Talk to button opens that agent's conversation with a message ready for you to change and send.
- Changed: An agent's request for a secret now shows only in its conversation. Left unanswered for 12 hours, or while the agent moves on to other work, it expires, and the agent can ask again if it still needs it.
- Changed: Links now stand out in blue, in conversations, files, a command's output and an agent's requests alike, so you can spot them at a glance.
- Changed: In Files, Show everything is now Show hidden files, which says what it does.
- Fixed: The Secrets panel beside an agent was empty. It now lists the secrets the agent has, with Give a secret… and Take back. The panels of other extensions with nothing of their own to show now say what the extension does and how it's set, instead of being blank.
- Fixed: Your Claude or ChatGPT plan could show the wrong name (Max for Claude Ultra, say), or the old plan after you upgraded. It now shows the name the provider gives it, and changes soon after you upgrade, without signing in again.

## v0.1.8
Released 2026-11-20 12:28 UTC.
- Added: Watch an agent's videos beside its conversation and comment on a moment or a stretch: click Comment above the video, then click or drag along the timeline. Drag across the frame too to point at part of it.
- Added: Comment on part of an image: click Comment above it and drag across the part you mean. Your comment stays on that part when the agent makes a new version.

## v0.1.7
Released 2026-11-20 11:41 UTC.
- Added: Comment on a file as you would on a document: select text in a file beside a conversation, comment on it, and send your comments to the agent in one go. It revises the file, replies and resolves them, and each comment stays with its text as the file changes. You can also suggest new text yourself, and accept or reject an agent's suggestions.
- Added: Sign in from any device with nothing to paste. When your Claktory runs on another computer, adding a Claude or ChatGPT subscription, or opening a sign-in link a command prints in a conversation, now finishes in the browser on the device you're using. Links in conversations also open when you click them.
- Fixed: After you approve a Claude or ChatGPT sign-in, the browser no longer says it worked before Claktory has finished it. Claktory shows how it went.

## v0.1.6
Released 2026-11-20 11:14 UTC.
- Added: Send files and pictures to an agent with your message: drag them into the window, paste them, or click the paperclip in the message box (on iPhone, from Photos or Files). The agent gets them in its folder, and sees pictures too when its model can.
- Changed: Open any project's files beside the conversation with Show files on its row in the sidebar, and a file you click opens right next to them. Move, resize or close them like any pane. On iPhone, touch and hold a project and choose Files. Focusing a project no longer opens its files.
- Added: Give your Claktory a name and color that every device shows. Right-click it in the rail, or go to Settings → General.
- Changed: Claktory stays quick on your iPhone or another computer: it sends far less over the network while agents work.
- Fixed: Your text no longer jumps while you type as an agent starts or stops working: Working and Stop now sit on their own line above the message box.

## v0.1.5
Released 2026-11-20 10:41 UTC.
- Added: See at a glance whether you're connected. The bottom of the sidebar always says Connected, Reconnecting… or Can't connect, and each Claktory in the rail shows a pulsing or red dot when it isn't connected. Click the line to see how you're connected, or to reconnect.
- Changed: Your messages show in the conversation the moment you send them, even over a slow connection. If one can't be sent, it stays there with Retry, so nothing you wrote is lost.
- Changed: Long conversations open quickly, even over a slow connection: Claktory brings their latest messages first, and earlier ones as you scroll up.
- Changed: Your Mac now keeps its connections to your Claktories in the Keychain, as your iPhone does, so other apps can't read them to pose as your Mac. They move there by themselves the first time you open Claktory.

## v0.1.4
Released 2026-11-20 09:25 UTC.
- Changed: A new device connects only once you allow it. When it uses its code, Claktory asks you, and both screens show the same four digits, so no one else can use a copied code. Closing Add a device cancels its code.
- Added: Archive a project you're done with, in its settings: it leaves the sidebar and its agents stop, with nothing deleted, and messages you send them wait. Find it under Archived at the bottom of the sidebar to unarchive it.
- Added: On the Map, hover an agent, or tap it on iPhone, for everything it has: extensions, folder, whom it can message and its secrets.

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
