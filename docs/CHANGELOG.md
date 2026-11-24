# Changelog

What each version of Claktory added and fixed, newest first. Claktory shows it in the app, and the Update banner shows what a new version brings.

Every change adds a line under `## Unreleased`: "Added …", "Fixed …", "Changed …", written for the person using Claktory. `scripts/changelog.sh stamp` turns Unreleased into the next version before a release, and `scripts/release.sh` refuses to release without it.

## Unreleased

## v0.1.20
Released 2026-11-24 14:30 UTC.
- Fixed: Updating from a Mac that uses a Claktory on another machine works on a slow or unsteady connection. The Claktory updates first, then the app downloads in pieces sized to the connection, waits when the connection drops and carries on by itself, and keeps what it has, so Try again picks up where it stopped, even after you quit the app.
- Added: Take an agent's conversation back to an earlier message: right-click it, or touch and hold it on a phone, and choose Go back to here, or ask Rivet to. The agent carries on from there, and Claktory tells it what the part after did, such as files it changed, since that still stands. What came after is kept as another version you can switch back to from Other versions, and going back to one of your own messages puts it in the box to change and send again. Rivet's changes show in History with its reason. (friend bug #9)
- Added: The Monitors page, under the alarm clock at the bottom of the sidebar, shows every agent's scheduled jobs, reminders and watches in one place. Filter by agent, project or state, pause or cancel any of them, open one to see each time it fired, and click an agent's name to go to its conversation.
- Added: Agents keep working while your Mac is plugged in with its lid open, so they carry on while you're away. A desktop Mac does this unless you turn it off in Settings; a laptop asks the first time agents work, and never stays awake on battery.
- Added: New models from Anthropic, OpenAI and OpenAI Codex show up in the model picker as soon as the provider offers them, without waiting for a Claktory update, and OpenRouter's list now comes from OpenRouter itself, so models it no longer offers leave it. Settings → Providers says when each list was last updated, with Refresh models to update it now, and you can type the exact name of a model the list doesn't have yet: Claktory checks it with the provider first. (friend bug #14)
- Added: On Android, share text, links, pictures and files from another app to an agent, or text to a chat. It goes into the message box for you to check and send; Send to… shows each Claktory's connection state instead of staying on Loading… while offline.
- Added: Gather pipes into groups in the sidebar, such as all your friends' feedback pipes under one row you can fold away. Choose Move to group… in a pipe's … menu. Pipes work just as before.
- Added: On iPad, draw on images as on PDFs: tap Draw, draw with Apple Pencil or your finger, then tap Done. Your drawing is saved and shows over the image on any of your devices, and goes with your answer when an agent asks you to review the image. Drawings on PDFs now show on your Mac and iPhone too.
- Added: You can send voice messages in chats too, the same way as to an agent: hold or click the mic, and Claktory turns what you said into text on the computer it runs on.
- Added: When your agents stop to wait for a usage limit to reset or for more credit, Claktory notifies you, once per provider, saying who's waiting and until when. Needs you shows them too, one item per provider, where you can try again now, use a usage reset, or continue them all on another provider such as OpenRouter.
- Added: Continue automatically, in Settings → Providers, keeps your agents working when they reach a Claude or ChatGPT usage limit by moving them to another provider you're signed in to, such as OpenRouter, without asking first. It's off unless you turn it on, since that provider charges you. (friend bug #8)
- Added: Agents and chats you moved with Continue on OpenRouter go back to Claude by themselves once your Claude usage has room again, so they don't keep spending on OpenRouter. Their line says when, and Back now still moves them sooner. Meanwhile a new chat, or a new agent on your default model, starts on OpenRouter too instead of waiting, and goes back with the others. (friend bug #8)
- Added: When agents wait on a usage limit, the provider you sign in to in Settings → Providers offers to continue them there, saying how many wait and which model they'd use.
- Added: When an agent or chat waits on a usage limit and you have a usage reset that refills it, Use a reset… sits beside what it waits for, before the option to pay another provider.
- Added: Agents with the Models extension can see how much of your Claude and ChatGPT usage limits is used and when each resets. Turn off Can change models in an agent's Models extension to let it see that without changing any agent's model. (friend bug #8)
- Added: Settings → Providers shows when a Claude account reaches Fable's own weekly limit, and which account your Fable agents use now, so a second subscription's state is clear.
- Added: When a newer version of a model your agents use comes out, Rivet tells you and offers to move them.
- Added: Change a chat's model from its … menu. A chat whose model can't run, such as after you sign out of Claude, offers Change model… so you can carry it on with another provider.
- Added: Rivet can compact an agent's conversation when you ask, and clear one with your approval: it shows you a card, and only Approve clears it.
- Added: Drag projects in the sidebar into the order you want, such as your most used at the top. On a phone, touch and hold a project first. The order is the same on all your devices. (friend bug #12)
- Added: An OpenRouter key's circle shows what it has to spend: what's left of its spending limit and what it spent today, and why it can't run paid models when it has no credit or its limit is used up.
- Added: When a pipe's agent is waiting for its usage or its daily spending limit to reset, the other side sees that it's waiting and until when, so they know when to expect an answer. Only the time is shared, never why. You see the same when their agent is waiting.
- Added: The Android app tells you when a new version is available, saying which version you have, with Download to get it from claktory.com. When the app is too old to connect, its screen has Download too.
- Added: On Android, Back closes what's on top first, on a tablet too: a dialog, then an expanded pane or a page such as Settings, and only then leaves the app. From Add a Claktory, Back returns to your Claktories, then to the one you were on.
- Added: Adding a device says where to get Claktory if it isn't on that device yet: under the code, Don't have the app? links to the iPhone and iPad app and the Android app, or the Mac app when you add a computer.
- Changed: Type a comment on a card and its buttons become Submit: the agent gets your comment as your reply, never as a yes or a no, and nothing is given, sent or connected until you approve a card as it is. Pressing Return in a comment field no longer approves.
- Changed: The iPhone, iPad and Android apps now start by saying Claktory runs on your computer, with Send me the link to get it there, and lead with signing in with your email. On iPad and Android tablets you can scan the code too, and Back goes from pasting a code or signing in to those first choices. Signed in with no Claktory on your account yet, the app says how to get one.
- Changed: When an agent asks for a secret, its card does it in one step: give the secret that fits, or paste a new one into the form the agent filled in, with a button that opens the page to make it on the service's own site. Rivet only steps in when you choose Let Rivet choose or a connected service can make one, and Do it here takes over while Rivet is held up.
- Changed: After Claktory makes an SSH key for an agent, its card says where to add the key, with Copy key and Done, which tells the agent it's in place. A file an agent asks to import has two buttons, Import and give to the agent, or Import only.
- Changed: To allow a new device, you now type the 4-digit number its screen shows, wherever you allow it, and three wrong numbers turn it away. A device waiting to be allowed also shows in Needs you, and your Mac notifies you, without the name the device gave itself. A Claktory on a server, which has no screen, is allowed with a command the new device shows under No screen on …?
- Changed: On iPhone, iPad and Android, images open as a preview sized to your screen, so they come much faster on a slow connection. On a slow connection, videos start as a smaller preview too, on any device. A Preview label says so: choose Show original for the full file, or zoom into an image and it loads by itself. Big PDFs open at once too, a page at a time, and you can search them once the full file has loaded.
- Changed: Charts look like Grafana's. Line charts use its classic colors starting with green, thin lines, gridlines both ways and a legend even for one line, and span the whole range you picked. Scatter charts show see-through dots with a ring, and number panels show the number sized to its panel, in its threshold's color, over a sparkline, or No data when there's nothing to show. Tap a series in the legend to show only it, or a chart or a dot to see its values; scrolling past a chart no longer opens one. On a phone, panels go full width, one under another, and at the largest text sizes the menus wrap instead of running off the screen.
- Changed: Claktory uses much less memory and writes far less to your disk. Long conversations open faster after a restart, one unused for an hour leaves memory until it's needed again, and each new message is saved on its own instead of the whole conversation again.
- Changed: Claktory uses less of your Mac's processor and battery while agents work: it redraws its window less often, and not at all while other windows cover it. Everything looks just as before.
- Changed: Your iPhone and iPad use much less data to stay up to date. Coming back to the app downloads only what changed while you were away, not everything again, so it's current sooner on a mobile connection.
- Changed: On a Claude Pro or Max subscription, agents send each screenshot or image they read once, instead of again with every step, so they upload far less and answer sooner on a slow connection.
- Changed: Choosing a model shows each model's provider beside its name, groups the list by provider and lets you search it, so Claude Opus 5 from Anthropic and from OpenRouter are easy to tell apart.
- Changed: Only Rivet hears when your computer runs low on memory or disk, and it tells you once, so your other agents no longer stop their work to warn you. If Rivet can't, Claktory notifies you itself. (friend bug #13)
- Changed: When an agent waits for a usage limit or for credit, its conversation says you can choose Try now to carry on sooner, such as after upgrading your plan or adding credit. An agent waiting for credit also carries on by itself when Claktory tries the key again an hour later.
- Changed: While Rivet waits on a usage limit, errors no longer offer to send them to Rivet, which couldn't read them until the reset. They say Rivet is waiting, with Continue on OpenRouter to get Rivet going now.
- Changed: Claktory emails you about unlocking your secrets only when it looks unusual, such as from a network it hasn't used in 30 days, not each time it restarts. Your account page at claktory.com still lists every unlock.
- Changed: Claktory uses far less network while the other end of a pipe is away: it tries to reach it less often, and still shows it online the moment it's back.
- Changed: Your iPhone asks whether Claktory may notify you right after you connect it, so you hear when your agents need you even if you only answer from it. Before, it asked only once you sent a message from it.
- Changed: On iPhone and Android, an agent's open files are listed newest first: the file it most recently showed you, or you opened, is on top.
- Fixed: Voice messages and answers you say are no longer lost on a weak connection. They wait on your device, marked "Not sent yet", and send by themselves when the connection is back, even after the app was closed. Messages you type that couldn't be sent, and your answers and decisions on cards and reviews, go by themselves then too.
- Fixed: An agent that loses the network mid-task, such as overnight or when your Mac wakes before its Wi-Fi, now waits for the connection however long it's down, and carries on by itself instead of stopping with a connection error.
- Fixed: On a slow connection, such as a weak mobile signal, big videos, images and PDFs no longer fail with "Connection lost". They load a piece at a time and show how much has come ("Loading video… 2.1 MB of 9.4 MB"), and if the connection drops, they carry on from where they got once it's back. Files and voice messages you send no longer drop it either, and still go when a fast connection turns slow.
- Fixed: Devices away from your network connect faster and more reliably, and one whose connection gets stuck finds another way within a few seconds instead of not connecting at all. Devices you added before keep connecting without being added again.
- Fixed: A device that reaches the Mac Claktory runs on at one of its other network addresses, such as its second network connection or from a virtual machine, now connects instead of waiting forever.
- Fixed: What needs you during quiet hours now waits for them to end. Then one notification tells you how many things need you, so you don't miss them in the morning. Tap it to open Needs you.
- Fixed: An update that your other devices' apps can't follow says so first, naming each device that won't connect until its app is updated. And your iPhone, iPad or Android phone no longer offers an update that would lock it out: it asks you to update its app first, on Android with Download for its new version.
- Fixed: When your Claude plan's weekly Opus or Sonnet limit is reached, only the agents on those models wait. Agents on other Claude models keep working on the same account.
- Fixed: An OpenRouter key that reaches the spending limit you set on it now moves your agents to your next key, or makes them wait until the limit resets, instead of stopping them with an error. A limit that lifts within moments is waited out quietly, without a notice.
- Fixed: Connecting an OpenRouter key with no credit, or with its spending limit used up, now says so and where to add credit, rather than that Rivet is ready.
- Fixed: Back to Claude shows as soon as Claude answers again, after Try now or when Claktory sees your usage has room, instead of at the old reset time, so agents don't stay on OpenRouter longer than needed. Settings → Providers stops saying the limit is reached then too.
- Fixed: Settings → Providers says an account is in use only while an agent or chat runs on that provider, so a provider you just added, such as OpenRouter, no longer looks in use while your agents are still on Claude. Its card says when no agents or chats use its models yet.
- Fixed: After Claktory restarts, your Claude and ChatGPT usage shows again as soon as your secrets are unlocked, instead of only after you choose Try now.
- Fixed: A chat waiting on a usage limit no longer looks like it's replying. Its row says what it waits for under its title, as an agent's does.
- Fixed: When a usage limit resets more than a day from now, Usage says how long in days and hours, such as "resets in 2 days 20 h, Thu 06:00". Before, it counted only whole days, so a reset on Thursday morning read as 2 days away on Monday. (friend bug #16)
- Fixed: Ask Rivet for a scheduled job or a reminder, such as checking your logs every 6 hours and telling you only when something's wrong, and it sets it up as a monitor for the agent that does the work. You hear about each new problem, not only the first, with a notification in case you're away. (friend bug #10)
- Fixed: Giving a secret to an agent with Give to… in Settings → Secrets now answers its request for that secret, so the agent carries on at once instead of waiting and asking again later.
- Fixed: A pipe's agent can now send the other side a file you attached to a message to it, such as a photo. A file you attached more than a day earlier waits for you to send it. (friend bug #15)
- Fixed: When a pipe's agent reaches its daily spending limit, the pipe now says so in the sidebar and under its header, where you can raise the limit, instead of showing Online while nothing gets answered.
- Fixed: After an update or a restart, Claktory shows the pipe you had open, with its messages straight away, instead of jumping to an agent.
- Fixed: Joining with a code that's expired or already used now says so, instead of waiting forever.
- Fixed: In Needs you, a task that comes one step at a time shows its next step as soon as you choose Done, on the Mac and on your phone, as it does in the conversation. Before, the step you'd done stayed until you closed Needs you and opened it again. (friend bug #11)
- Fixed: Signing in to your Claktory account on a phone or tablet now also confirms that device is yours, so you're no longer asked to sign in again to confirm it.
- Fixed: When a Claktory stops allowing this device, the app now says so even with a conversation open, and tells you how to add the device again. What you were typing is kept for when it's allowed back.
- Fixed: Right after you open Claktory on a slow network, your Claktories no longer show "Can't check whose it is" while it's still reaching your Claktory account. It now says "Couldn't check your account" only if that fails for 30 seconds, and a device that's signed out says so, with Sign in.
- Fixed: On iPhone, the connection word under your Claktory's name in the menu now changes to "Connected" as soon as it connects.
- Fixed: Panes no longer squeeze the conversation to a few letters a line. It always keeps room to read: a pane that wouldn't fit beside it closes the one opened longest ago, and in a narrow window Settings, Usage and the other pages open in the conversation's place. On an Android tablet, Back closes the panes beside the conversation, the newest first, before it leaves Claktory. Narrow Android tablet windows use the phone layout, so Settings opens as a page instead of squeezing the message box.
- Fixed: Charts draws new data right away, even if it came while Claktory's window was hidden; before, a chart could keep its old line until you reopened the app. With Reduce motion on, charts no longer animate.
- Fixed: On the Mac, Claktory follows Reduce motion in System Settings → Accessibility → Display: the working waves and the characters' faces hold still, as they already did on iPhone, iPad and Android.
- Fixed: On Android, Settings → Notifications says this phone doesn't get notifications yet, instead of looking set up. And Send a test notification, on every device, names the devices it went to, or says none gets notifications yet, instead of just "Sent."
- Fixed: On Android in light mode, the clock, Wi-Fi and battery at the top of the screen are now dark, so you can see them on Claktory's light screens. They turn light with the dark appearance.
- Fixed: On an Android tablet, the message box and Send stay above the on-screen keyboard, so you can see what you type. On the first screen, Connect stays in sight while you paste a code.
- Fixed: On a phone on its side, the message box no longer disappears behind the keyboard while a banner asks you to confirm the phone is yours. The banner steps aside while you type and comes back when the keyboard goes.
- Fixed: On iPad with a hardware keyboard, Return sends your message at every window size. Before, in a narrow Split View or window, it started a new line.
- Fixed: On iPad, when a PDF you've drawn on changes, the note about drawings from the earlier version names only pages that still have a drawing, not a page you erased.
- Fixed: On iPhone and Android, tapping Review on a card in Needs you shows the document right away, on top, and Back returns to Needs you. Before, from the Needs you button at the top of a conversation, it opened behind Needs you and seemed to do nothing.
- Fixed: On a phone, tapping a file's name where an agent showed it now opens the file, as Files in ⋯ does.
- Fixed: On a small iPhone at the largest text sizes, the line above the message box shows what an agent waits for, and why a tap did nothing, rather than only its buttons. It stays whole when you change the text or display size while it shows.
- Fixed: A damaged voice recording now says so plainly ("Couldn't read the recording. It may be damaged.") instead of showing technical details.
- Fixed: Team health no longer calls new or on-request agents idle. A new agent is judged only on the time since it started, and one that answers what wakes it isn't idle, however rarely it's woken.

## v0.1.19
Released 2026-11-23 14:16 UTC.
- Added: Tell Rivet what you'd like from an app, such as a chart of an agent's bugs, and it sets the app up, asking you first only before an app reads anything private or can reach the internet. Every change shows in Needs you's History and in Marketplace → Apps with who made it, and Undo puts it back.
- Added: When your agents wait on a Claude or ChatGPT usage limit and you're signed in to another provider such as OpenRouter, choose Continue on OpenRouter to keep them working on the same model, or the closest one, for one agent or all of them. Once your usage has room again, Back to Claude switches them back.
- Added: Ask Rivet to put an agent on a model, such as "set Lea to Opus on OpenRouter", and it finds the exact model on the providers you're signed in to and changes it in one step. The change shows in Needs you's History with Rivet's reason, and Undo puts the agent back. Rivet also tells you when a provider can't take requests right now, such as when it's out of credit.
- Added: Charts can chart a CSV or TSV file in your projects, such as a list of bugs, and updates as the file changes. Ask Rivet for it, and you approve each file first, or pick one yourself with Add a file… in Marketplace → Apps.
- Changed: An agent that reads a pipe no longer needs your approval each time a command of its uses a secret. It still treats what a pipe brings as the other side's words, and Claktory still never sends your secrets through a pipe.
- Fixed: Pipes no longer pause after a few messages between agents. A pipe now pauses only after 50 messages in a row between the agents with you not writing, or when they keep sending each other the same long message; short replies such as "OK" or "Thanks" said again no longer pause it.
- Added: Add another AI provider from the + beside the usage circles at the bottom of the sidebar.
- Added: OpenRouter now offers Claude Opus 5.5, Claude Sonnet 5.5 and GPT-6.1 Sol.
- Added: Copy a whole message in one click. On a Mac or an iPad with a pointer, hover over an agent's reply and click the copy button beside it, or right-click any message and choose Copy message. (friend bug #5)
- Changed: Apps are in the Marketplace's Apps tab, with Claktory's own first, and opening one such as Charts fills the window; Back, Collapse or Esc takes you back. Charts comes with Claktory and reads nothing until it's set up. If you uninstall Charts, it stays uninstalled.
- Changed: A recording under 5 seconds with nothing said in it, such as a mic tapped by mistake, goes away quietly, as a voice message or an answer by voice, instead of saying no words were caught. A longer silent voice message still says so, in case your microphone isn't working.
- Changed: The top of an agent's conversation no longer shows how many quiet copies or quiet messages it has, so the header keeps its usual height on iPhone. Quiet copies are still listed in the agent's Messaging panel, and a quiet message still shows at the end of the conversation until the agent reads it.
- Fixed: Agents waiting for your Claude or ChatGPT usage to reset carry on as soon as you sign in again, such as after upgrading your plan, instead of waiting for the reset, and Try now beside the wait checks again right away. If you're signed out while an agent works, it waits for you to sign in, with Sign in… beside it, instead of stopping. The notice in its conversation says when the limit resets as a time of day, so it stays right when you read it later. (friend bug #3)
- Fixed: A pipe's agent is now told to pass on what you write to it only when you ask. The line under the box no longer says only the agent sees your message: it says the agent can pass it on, or that you approve it first when Review outgoing is on. (friend bug #1)
- Fixed: If you refused the camera when connecting your phone, Scan code instead now goes back to scanning, with Open Settings to allow the camera. To join a pipe, you can also scan its invite: the person inviting you chooses Show QR code, and you choose Scan code. On a phone or iPad, Connect to another Claktory has Scan code too, and so does moving Claktory to another computer. Pasting another kind of Claktory code where you connect, such as a pipe invite, now says what it is and where to get the right one, and a code pasted from a messenger, broken over lines or among other words, now works. (friend bug #2)
- Fixed: What you type or change on a card waiting for you stays when you scroll the conversation away and back: an answer to an agent's question, what's in the way of a step, a comment, or a password's length, a name or a value on a secret request. Before, it went back to empty or to what the agent asked for.
- Fixed: The Checklist panel keeps items an agent added at once in the order it added them, even when there are dozens.
- Fixed: Stop reading reports… works once no open pipe or project's new pipes send an agent reports: a closed or deleted pipe no longer holds it, and when one still does, Claktory says which.
- Fixed: On a Mac, Needs you, Settings and the other pages open while a pipe or chat is showing, in its place; before, they opened out of sight beside the last agent you chose.
- Fixed: A pipe invite no one used says it expired as soon as its time runs out, instead of waiting until Claktory restarts, and Cancel invite goes away with it.
- Fixed: Needs you's History says only what you changed the first time you save a pipe's report settings, and says a report type lets more of their words through only when a field for their words is added or widened, not for a new choice.
- Fixed: When a device loses its connection during a move to another computer, it says so instead of showing the last step it heard and a Cancel move button that can't work. Once the move finishes, it connects to the new computer by itself.
- Fixed: When a move Rivet started fails at the very end, Claktory still says it was Rivet's move, and to which computer, after it starts again, instead of a general message.
- Fixed: Making a pipe invite says Making… while it works, and a second tap no longer closes the invite before you've seen its code: Copy code takes the place of Make invite, and Done opens the pipe. A copied invite code stays out of clipboard previews, keyboard suggestions and clipboard history, and is cleared from the clipboard once it stops working, even if you left Claktory or quit it in the meantime.
- Fixed: When you search a conversation or a file, a match far along a long line, in a tool call, a block of code or a wide table, scrolls into view instead of staying past the edge. Switching a file between Formatted and Plain keeps the match you're on in view. Search also finds the messages still waiting at the end of a conversation, quiet ones included. On iPad, Return in the search field goes to the next match. With Needs you or Usage open beside it, the magnifying glass at the top searches the conversation again, not the pane.
- Fixed: On iPad without a hardware keyboard, the message box has a Send button, so you can send with the keyboard on screen, whose Return starts a new line. With a hardware keyboard, Return still sends.
- Fixed: On iPad, a file an agent shows you comes to the front, even one that's already open. Before, when the window had more panes open than it could show, the file stayed out of sight.
- Fixed: In a PDF in a narrow pane on iPad, tapping a search result shows the match marked on its page, high in view. Before, the marks went as the results closed. Tap the magnifying glass to go back to the results.
- Fixed: When you search a PDF, the yellow marks cover just the words you searched for, even at the start of a line, and the match you're on is in sight, not hidden under the find bar on a Mac or the toolbar on an iPad. On iPhone and iPad, the page number names the match's page until you scroll.
- Fixed: At the largest text sizes, Settings puts each label above its setting when there's no room beside it, so words no longer break in the middle. Numbers, durations and your devices show whole, and a Claktory's initials stay inside their tile. Your recovery key's sheet also scrolls, and the key wraps so you can see all of it. Tool calls in a conversation, Talk to buttons, panel titles and the Back buttons above a conversation or a full-window view grow with the text instead of cutting it in half, and text folded under Show more still shows its first few lines.
- Fixed: In a narrow pane, a file's name stays in its header, a tool's name no longer runs over how long it took, and "Showing" names the file it shows.
- Fixed: Switching a file between Formatted and Plain clears what you'd selected, instead of highlighting other words.
- Fixed: After you import a large file into your secrets, Claktory no longer stops answering for many seconds each time a secret changes. A file of exactly 1 MB can be imported too, and a password that's too short is described correctly ("An 8-character password").

## v0.1.18
Released 2026-11-22 22:32 UTC.
- Fixed: A card asking you to add a secret for an agent no longer stays in Needs you after that agent withdraws its request. It closes and shows how the request ended, and a card already stuck this way clears when Claktory updates.
- Added: Claktory for iPad, made for iPad rather than as a larger iPhone. Files and pages open in panes beside an agent's conversation, which you can resize and rearrange, full screen, in Split View or in Stage Manager. Use a trackpad and keyboard (hold ⌘ to see the shortcuts), and draw on PDFs with Apple Pencil or your finger: drawings save to your Claktory, so they're on all your iPads, and you can answer an agent's review of a PDF with them.
- Added: Apps, starting with Charts, which comes with Claktory: live charts of what your agents do, such as their runs, messages and tokens, which you and your agents can change. Install it in Settings → Apps, choose what it may read and which agents can use it, and pin it to the dock at the foot of the window, or to the drawer on a phone. Each app runs walled off and reaches only the sites you approve.
- Added: A pipe can forward reports to one of your agents: bug reports, task requests, questions and more, or report types you make in Settings → Pipes. The agent you choose reads them and keeps track of them, with no way to run commands or reach anyone; set it in the pipe's settings, or ask Rivet.
- Added: Search a whole conversation, chat or pipe, not only what's on screen. Press ⌘F, or click the magnifying glass at the top of a conversation; on iPhone and Android, tap Search in the ⋯ menu. ⌘F also searches a file you're viewing, Needs you, Settings and an agent's panels.
- Added: Answer by voice. Use the mic in an answer, a review reply, a checklist reply or a comment, and what you said fills the field, for you to check before you send.
- Added: Pipes show whose Claktory is on the other side: when you're both signed in to claktory.com, the pipe shows Verified with their email, and warns you if that ever changes.
- Added: A pipe's agent has a daily spending limit, $10 unless you change it in the pipe's settings. Past it, messages from the other side wait until midnight, so they can't run up your costs; yours still reach the agent.
- Added: An agent can ask for a new password for a service, and Claktory makes a strong one: you see how long and how strong it'll be, choose Generate, and it's saved in Settings → Secrets without anyone ever seeing it, the agent included.
- Added: An agent can ask to import a file from this computer into your secrets, such as a signing key. You see exactly which file it is, choose whether the agent gets it, and Claktory keeps it as it is, with no copying and pasting.
- Added: You can report an agent's reply to Claktory's team. Right-click it on the Mac, or touch and hold it on your phone, and choose Report. Only that reply is sent, with a note if you add one.
- Added: Reply on a checklist item right where it is, in the Checklist panel. The agent knows which item you mean.
- Added: The Android app now updates with every Claktory release. Like the iPhone app, it connects when you scan your Claktory's code or open its link.
- Changed: Agents ask you one thing at a time, and a longer task comes one step at a time. Do a step and choose Done to see the next; if you can't, choose Can't do this and say what's in the way.
- Changed: Needs you shows only things you can answer. A checklist item waiting on you shows there once the agent asks you for something about it (an answer, a secret or an approval for Rivet), one entry each, instead of as a row with nothing to answer.
- Changed: When an agent asks you to review a file, answer right where you're looking at it: type below the file and press Return, or Approve. You're back where you were when you're done.
- Changed: When an agent shows you a file and your panes are full, it replaces the one that's been open longest instead of asking you to close one. The pane you've expanded to fill the window is never replaced.
- Changed: In a pipe with its own agent, your messages to that agent are marked "To Kai only", so it's clear they didn't go to the other side.
- Fixed: A pipe no longer goes unanswered without you knowing. If the agent that answered it is unticked or deleted, the pipe's own agent or another agent limited to the pipe takes over, the pipe says who, and it catches up on what it missed. If none is left, new messages wait for you in Needs you until you choose one in the pipe's settings.
- Fixed: Pasting a code under Your Claktories, or when setting up a Mac to use another Claktory, now asks "Connect to this Claktory?" and shows its code to check first, as scanning a code and opening a link already did.
- Fixed: Text in a conversation can't be disguised. Characters that reverse or hide words, such as in a file's name, show as codes like \u{202E} in what an agent's tool calls show; in messages and in what agents ask you to decide, characters that reverse words do too.
- Fixed: On iPhone and Android, Needs you, new copies and quiet messages show at the top of a conversation instead of over what you're reading. The Latest button also moves aside while you comment on a step, so the comment stays in view above the message box.
- Fixed: On iPhone, an agent limited to a pipe says so under its name at the top of its conversation, as on the Mac, and one that reads a pipe's reports shows Reads reports there: tap it to see them.
- Fixed: You can select and copy any words in Needs you and its History, such as a secret request's description, to talk about them with an agent. Copied text keeps its lines, and messages and requests no longer run together.
- Fixed: Selecting text with the mouse or trackpad lands where you point when Claktory is zoomed in or out, however far you've scrolled, and the view no longer scrolls away while you drag. Copying several lines of a file in Plain keeps its line breaks and blank lines.
- Fixed: When you type a comment on a video, Space types a space instead of playing or pausing it, and a viewer's own keys never take what you type in a field. Escape in an expanded file's answer leaves the field and keeps your words, instead of closing the file.
- Fixed: In a PDF on the Mac, Search and ⌘F open the same find bar as a conversation's, over the page. It keeps your typing once matches show, and its arrows, ⌘G and ⇧⌘G go to the next and previous match.
- Fixed: In a PDF on iPhone, the words you search for are marked in yellow, and the arrows take you to each match in turn, marked apart from the rest. The marks go when you tap Done, instead of staying on the page in red and piling up with each search.
- Fixed: On a Mac whose sound output is also a microphone, such as a USB headset or a dock, opening a video no longer asks for the microphone. If macOS asks at all, it's when you press Play and the sound starts.
- Fixed: On a phone with large text, app charts stay readable: side-by-side panels stack, and labels, titles and table headings wrap without overlapping. Apps also stop refreshing while Claktory is in the background.
- Fixed: At the largest text sizes, the count of what needs you is no longer cut off at the top and bottom.
- Fixed: Zoomed out, a file you've expanded to fill the window keeps its Back button clear of the window's close, minimize and zoom buttons.
- Fixed: The Comments extension shows its icon instead of a letter C.

## v0.1.17
Released 2026-11-22 12:51 UTC.
- Fixed: On iPhone, notifications could stop arriving, with nothing to say why, when the iPhone couldn't finish setting them up with your Claktory. Now it tries again each time it connects, and Settings → Notifications says when it couldn't, with Try again.
- Fixed: Agents' web searches fail far less often. When a search engine shows a CAPTCHA or stops answering, Claktory tries the next of several free engines, such as DuckDuckGo, Bing and Yahoo, and favours the ones that have been answering. When none can answer, the agent learns when to try again.
- Fixed: The Claktory app on a Mac with an Intel processor couldn't connect to your Claktory running on another computer, because part of the app failed to load.
- Added: Turn on Advanced diagnostics in Settings when something keeps going wrong, such as connecting: Claktory and your devices record how it happens, so your agents can find the cause. It stays on your Claktory, never includes messages, and is kept for 14 days.
- Changed: A key the vault generates for an agent no longer asks you each time it's used, unless you turn on Ask each time when you approve it.
- Added: Usage shows how many agents were working at once, above the token chart, so you can see when they were busiest.
- Added: Check for updates in Settings looks for a new version of Claktory right away, so you don't have to wait for the Update banner. It tells you whether you're up to date, a new version is on its way or ready, or why it couldn't check.
- Added: Agents can send each other quiet messages that don't interrupt: the other agent reads them the next time it starts working, so less time and money goes on interruptions. An agent shows how many quiet messages are waiting for it.
- Fixed: Moving Claktory to another computer over the network stopped at any project folder outside your home folder, such as one on another disk, and suggested an option that move doesn't have. Now that folder moves into your home folder there, in a folder named after this computer. Rivet tells you where before you confirm, and can leave a folder out if you ask.
- Fixed: On iPhone, tapping a file you sent in a conversation opens it again.
- Fixed: On the Mac, when Settings, the Marketplace or What's new is squeezed into a narrow pane, its text no longer looks cut off at the edge. The edge fades out to show there's more, and you can scroll sideways to see it.
- Fixed: On iPhone, at the largest text sizes, a page's title, such as Notifications, no longer breaks in the middle of a word.
- Added: Ask Rivet to move your Claktory to another computer, and it does the move for you. For a server, add the key Rivet gives you and it sets the server up itself; for another Mac, paste one line in Terminal there and give Rivet the code it shows. You still confirm the move. Afterwards Rivet tells you how it went: whether your secrets are unlocked, which devices have connected, whether your sign-ins work and your project folders are in place, and what your agents used that the new computer doesn't have.
- Fixed: A pipe could stop sending after you approved it, or start again after you paused or closed it, if the other Claktory was talking at that moment. A new pipe could also close by itself right after it was joined, saying the other side had closed it. Pipes now keep what you chose.
- Fixed: Messages still waiting to go when a pipe closed, from either side, said "Not delivered yet" for ever. They now say they weren't sent because the pipe closed.
- Fixed: An agent can no longer land work that would quietly undo what other agents landed in the meantime. Claktory names the files and says how to fix it, and lands a revert only when the agent says it means one.
- Fixed: On iPhone, at the largest text sizes, the top of the menu no longer pushes your agents off the screen. It scrolls on its own instead.
- Added: On iPhone, scan a Claktory's code with the Camera app to open Claktory and connect. Every way of connecting, whether a scan, a link or a pasted code, now asks you first and warns you when that Claktory isn't on your account.
- Added: PDFs open on the Mac and iPhone: scroll and zoom, select and copy text, and find words, with ⌘F on the Mac or Search in More on iPhone. A link in a PDF asks before it opens.
- Added: Choose what Claktory notifies you about in Settings → Notifications: which kinds, how much a notification shows, quiet hours, and a test notification. Turn notifications on or off for a single agent in its settings.
- Fixed: On iPhone, the buttons on a card, such as Dismiss and Approve, stay together at the right edge, under your thumb, instead of stacking up beside their words. When they can't share a line, at the largest text sizes, each spans the card's width.
- Added: Add a comment when you approve or dismiss anything an agent asks of you, such as a question, a secret or a proposal. The agent gets it with your answer.
- Added: Checklist items say what they're waiting for: your decision, something for you to do, another agent, or something outside. Items waiting for you show in Needs you even when no one asked, an item opens again by itself once what it waited for is done, and work set aside for now shows as Parked instead of Blocked. In Settings → Checklist, choose how soon Claktory follows up on blocked items.
- Fixed: On iPhone, Claktory could show Reconnecting… until you quit the app and opened it again, after the phone was locked for a few minutes, say. Now it reconnects within seconds when you come back, and starts over by itself if that doesn't work. To reconnect right away, tap Reconnecting… or Can't connect above the message box (click it on the Mac). When your Claktory can't be reached at all, conversations on any device say so and what to do.
- Added: Work moves faster between agents: one can hand work straight to the next and leave its lead a copy that doesn't interrupt it, so the lead still sees what happened without passing everything on. You see the copies in the lead's Messaging panel, and an agent's message says what it's replying to.
- Added: Needs you has a History tab with everything you answered, approved, declined or dismissed in the last 30 days. Search it by agent or words, and click or tap one to go back to it in its conversation.
- Fixed: Build caches in a busy agent's checkout could grow past their limit in Settings → Resources and fill your disk, because Claktory cleared them only while the agent was idle. Now it also clears them while the agent waits on its model, never during a build.
- Fixed: A pipe you joined without naming it took the name the other side gave it, which could pass for Claktory's own, such as "Claktory Security". Now it's called "Pipe joined 22 Nov" until you rename it.
- Fixed: When Screen Time or your organization blocked the microphone on iPhone, Claktory sent you to Settings, where you couldn't allow it. Now it says the microphone isn't allowed on this device.

## v0.1.16
Released 2026-11-22 03:22 UTC.
- Added: Your Mac and iPhone tell you when an agent needs you or answers you: the iPhone even when Claktory is closed or the phone is locked, the Mac while Claktory is open, even in the background. Click or tap a notification to go straight to that conversation. On iPhone, what it says is private and encrypted all the way to your phone.
- Added: An agent can ask you to review a document. Click or tap Review on its card to open the document beside the agent's conversation; your comments there go straight to the agent, and you finish by approving or dismissing the card. On iPhone the document opens as a page, with Dismiss and Approve at its foot.
- Added: Goals on an agent's checklist: tell it the outcome you want, or add one yourself in its Checklist panel, and see how far each goal's items are. An agent can also check in on its goals and unfinished items as often as you choose, from every 15 minutes to once a day, to see what it can move forward.
- Added: A checklist item can say what it's blocked on, and one blocked on you says how long it has waited and whether you were asked. When work waits on you and its agent hasn't asked, Claktory reminds the agent once a day to ask you, and Needs you shows how long each item has waited.
- Added: Rivet can check how your agents work together, find what slows them down or costs the most, and suggest changes for you to approve on one card. When a problem lasts, such as work waiting on you that no one asked about, it speaks up by itself. It looks only at counts and times, never at what your agents wrote.
- Added: Messages now show when they were sent, in small type at the corner of each bubble, and a conversation marks where each day begins.
- Added: Expand any pane beside a conversation to fill the window, with the button in its header; Back (or Esc) returns everything exactly as it was. It stays expanded after a restart, and anything an agent opens meanwhile waits for you.
- Changed: When an agent thinks several times in a row, its thoughts now show as one line, such as "Thought 5 times", instead of a long stack. Click or tap it to see each one.
- Added: See at a glance how many of your agents are working: the sidebar shows it, such as 4/23, beside the usage circles. Hover it to see who (on iPhone, touch and hold), or click or tap it to open the Map.
- Changed: Projects and Chats are now two sections of the sidebar, and you can always see both, however many projects or chats you have. Click either heading (on iPhone, tap it) to fold it away or open it again; folded, it shows how many it holds and whether anything in it needs you.
- Added: Drag a project's folders in the sidebar to put them in the order you want; on iPhone, touch and hold a folder first. A project's pipes now sit right under the project, so they're easy to reach however many agents it has.
- Added: A pipe's liaison now belongs to its pipe. The pipe opens on the liaison's conversation, with the pipe's messages in it as cards: write to the liaison, or choose the other Claktory above the box to message them yourself. The liaison has a name of its own, its face is on the pipe's row in the sidebar instead of a row of its own, and deleting the pipe deletes it too, keeping its conversation in the pipe's folder.
- Added: Ask Rivet to set up a pipe for you: it shows a card where you invite someone and copy the code to send them, or paste a code you were sent to join their pipe. Rivet never sees the code.
- Added: Review incoming holds each message from a pipe's other side until you let it in, before your agents read any of it. Turn it on, or Review outgoing to check what your agents send, as you approve a pipe or in its settings. New pipes start with both off; pipes you already have keep what they had.
- Added: For secrets that matter most, choose Ask each time in Settings → Secrets: every command that uses one waits for you to allow it, with the exact command shown.
- Added: An agent can ask for a new SSH key. Approve it on its card and Claktory makes the key itself, so no one ever sees its private half, you and the agent included: the agent gets only its public half, and the key goes to the agent or to no one, as it asked.
- Added: When you give an agent a secret it asked for, you can add a note for it, such as what to check.
- Added: Needs you reminds you to save your recovery key, which unlocks your secrets if claktory.com can't, and checks that you kept it by asking you to type part of it back.
- Added: When Rivet moves your Claktory to another computer for you, Claktory asks you itself, on every device and in Needs you, to check the digits and confirm. Only you can confirm a move.
- Added: After Claktory moves to another computer, the Mac it left says where and when it went, and Delete this copy… removes what's left there once you're sure the new one works.
- Added: With more than one subscription to a provider, a dot under its circle in the sidebar shows which one Claktory is using now. Use this one, on the Usage page or in Settings → Providers, puts a subscription first, so Claktory uses it whenever it has room, including after a reset.
- Changed: Coming back to a conversation after a break costs less: Claude now keeps conversations ready for an hour instead of 5 minutes, and a long one left idle is summarized just before that hour runs out. To go back to 5 minutes, change Keep conversations ready in Settings → Providers.
- Changed: Coming back to a conversation after a break now costs less on OpenAI's newest models (GPT-5.6 and later) too: a long one left idle is summarized just before OpenAI stops keeping it ready. Settings → Providers says how long each provider keeps conversations ready.
- Changed: When a provider stops accepting one of your subscription sign-ins, Claktory moves your agents to your next account at once and asks you to sign in again, in Needs you and Settings → Providers.
- Changed: Your providers' sign-ins are now kept encrypted with your secrets. While your secrets are locked (after a restart when claktory.com can't be reached, say), agents that need a model wait and carry on by themselves once they're unlocked, and you can still sign in to a provider.
- Changed: An agent now opens at most 4 panes beside its conversation, counting the ones you opened yourself, such as Settings, then asks you to close one. Panes you open aren't limited this way. Change how many in Settings → Display.
- Changed: On iPhone, the menu starts with Needs you, as on a Mac: one row with how many things wait, which opens them all on one page. Projects, folders and agents nested under others are indented less, so their names have more room.
- Changed: Before you choose an AI provider, Claktory now says plainly that your agents send it your messages and the files and pages they use.
- Added: On iPhone, you can delete your Claktory account and read how Claktory uses your email right from the sign-in screen, before you've connected to a Claktory.
- Changed: To help find problems sooner and see what to improve, Claktory's hourly check-in with claktory.com (while you're signed in) now also counts your pipes, secrets and monitors, your kinds of devices and how often they connect, how far setup got, and how often an update had to be undone or Claktory stopped unexpectedly. It sends only counts: never names, prompts, conversations, files or what's in your secrets.
- Fixed: When something new came into Needs you while you were typing an answer, what you'd typed was lost and the cursor left the box. Now your answer and your place stay as the list changes.
- Fixed: When Claktory turned down something you did, such as sending a pipe message or dragging an agent under a lead, the app could miss it and not say why. Now it tells you why, and an unsent message comes back to the box.
- Fixed: Your agents could still read a pipe message you discarded, or one waiting for you to let it in. Now they see only what you let in; one that waits shows only that it's waiting.
- Fixed: Moving Claktory with a code that didn't really come from your other computer could skip the check that both computers show the same digits. Now you always confirm the digits of a new move.
- Fixed: A phone or Mac you just added to a Claktory could show Connecting… for minutes when it couldn't connect directly; it connects at once now.
- Fixed: Connecting a pipe gave up for good when the other Claktory couldn't be reached on the first try — while a Mac was still asking about the local network, say. Now it keeps trying until the code expires. The pipe says why it can't get through yet, and an offline pipe says since when and why.
- Fixed: When you let in pipe messages out of order, the agent that answers the pipe could miss the earlier one. Now it hears of every message you let in.
- Fixed: When a search engine blocked your computer, web search kept asking it, so the block could last for hours. Now Claktory pauses that engine, even across restarts, and agents searching for the same thing at once share one search.
- Fixed: If an update ever had to be undone, the earlier version could fail to read the settings the newer one saved. Now it reads them, keeping what it doesn't know untouched for when you update again, and projects inside Claktory's own folder keep working wherever Claktory moves.
- Fixed: One provider sign-in that Claktory couldn't read (edited by hand, say) hid every provider's models. Now only that provider's models are missing.
- Fixed: A provider account you renamed right after signing in could get its old name back a moment later.
- Fixed: A private key kept as a secret had its first and last lines ("-----BEGIN … PRIVATE KEY-----") hidden everywhere, even around other keys, which then showed under your key's name. Now only your key itself is hidden.
- Fixed: A message from another agent showed its sender's role twice ("Sam (Backend) Backend"), and replying to it quoted "Sam (Backend) wrote:". Now the name and role show once, and the quote says "Sam wrote:".
- Fixed: On iPhone, copying an agent's message (touch and hold it, then tap Copy) gave you its formatting symbols, such as ** and #. Now you get the text as it reads, with lists and tables kept in shape.
- Fixed: On iPhone, Settings → Providers squeezed each account's buttons beside its email, cutting the email short. Now it shows in full, with the buttons under it.
- Fixed: On iPhone, a form's button at the top could be cut short when its words were long, such as "I've saved it" on a small iPhone. The title now moves under the buttons so they show in full.
- Fixed: When an agent set up Claktory on a server and connected your Mac to it, the connection could get stuck and never finish. Now the agent sees the digits to check, allows your Mac, or tells you why it couldn't connect.
- Fixed: Installing Claktory on a Linux server as an ordinary user said it had failed because another Claktory was using its port, even though Claktory was installed and running. Now it reports that it worked.
- Fixed: Right after you installed Claktory on Linux, `factoryd status` counted a device that hadn't connected yet. Now it counts only devices you've allowed in, and says how many are waiting for you to allow them.

## v0.1.15
Released 2026-11-21 20:50 UTC.
- Changed: A pipe's liaison agent is now limited to the pipe: it can only read and answer it. It can't run commands, reach your other files, or use secrets, the web or your other agents, so a misleading message from the other side can't get further. Liaisons you already have are limited too; to lift it, turn on Can run commands on this computer in the pipe's settings.
- Changed: The sidebar shows who reports to whom: each agent's reports sit under it. Drag an agent onto another to make it report to that agent, or choose its lead in its settings; as you drag, it says what letting go will do. Agents you'd nested only to arrange them are back at the top level: drag them under their lead again, or ask Rivet.
- Added: When an agent names another with @ (@Kenji), the conversation shows a tag with that agent's character, name and role. Click or tap it to open its conversation.
- Added: In a busy conversation, an agent's answer shows a quote of the message it's answering, so you can tell what it replies to. Click or tap the quote to go to your message.
- Changed: The sign-in code now shows as six boxes, one per digit. Type it, paste it from your email, or tap the code your iPhone offers above the keyboard: it fills all six and signs you in.
- Changed: Adding a computer as a device now gives you a code to copy and paste in Claktory there, instead of a QR code it can't scan. In Settings → Devices → Add a device, choose Phone or tablet or Computer.
- Added: When an agent sets up Claktory on a server for you, it connects your Mac to it too, with no code to paste, and your Mac tells you once it's done.
- Added: An agent can move another computer's Claktory onto the computer it runs on: it shows you the move's digits to check, and goes on only once you say they match.
- Changed: On a server, a setup script or an agent can add only your first device. After that, allow new devices from one that's already connected, or run factoryd allow in a terminal.
- Changed: A file you're reading beside a conversation is no longer swapped out when its agent shows you another. An agent can show up to 5 files at once, then asks you to close one.
- Fixed: A pipe could be misused in a few ways, now closed: allowing an agent a secret covers only the command you saw; a file that changed after you looked at it isn't sent; an agent that hears from one that read a pipe asks you before it uses a secret too; the other side's description of itself is cut to one short line; and files over a pipe's limits wait outside the inbox until you let them in.
- Fixed: An agent waiting for usage could stay asleep after usage was back. Using a usage reset now wakes it at once, even on Fable, and messaging a waiting agent makes it try again right away.
- Fixed: You couldn't select or copy the text of an agent's question, so a command it asked you to run had to be typed out. Now you can select its text anywhere it shows, and a command in it has a Copy button.
- Fixed: The scrollbar in a conversation jumped around and changed size while you scrolled. Now it moves smoothly with you.
- Fixed: Pasting a garbled code to connect to a Claktory, or opening a garbled link to one, made the form disappear. Now it says it isn't a Claktory code, so you can paste the right one.
- Fixed: After updating from v0.1.13 or earlier, voice messages could say speech recognition isn't installed. Claktory now adds what's missing the next time it starts.

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
