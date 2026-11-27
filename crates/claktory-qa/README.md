# claktory-qa

Throwaway Claktories ("sandboxes") to test a feature in, headless, and a
driver that speaks the protocol to them. It's Theo's QA sandbox
(`factory-research/qa-sandbox.md`) made repeatable: QA writes numbered user
paths for a feature, runs the ones the backend can show here, and reports each
path with the JSON it saw.

Each sandbox is a real factoryd built from a commit. It has its own fresh
home, a free port from 7500 up, and its own account service whose emails land
in a folder. Starting one takes about 2 s, and ending one leaves nothing
behind.

## Safety

claktory-qa refuses rather than reaching anything real:
- **Folders:** it never uses the real Claktories' own files in `~/.factory` or
  `~/.factory-dev` (settings, data, binaries, releases). Agents' folders and
  checkouts there are fine.
- **Ports:** never 7420 or 7421.
- **Network:** never claktory.com. A sandbox's account service is its own, on
  this computer, and its tunnel's relays point nowhere, so pipes and devices
  between sandboxes connect directly. The driver refuses a service or relay
  that isn't on this computer.
- **Models:** no real model. An agent in a sandbox would run as you, with this
  computer's files, so `StartLogin`, `cli login` and `cli provider` are
  refused. For agents that run, start the sandbox with `--model`: they use
  qa-model, which answers from a script you write (see "Agents that run"
  below). With `--claude`, Claude is signed in too, fake, on qa-model (see
  "Claude at its usage limit").
- **Environment:** factoryd starts with an empty environment (no API keys, no
  `FACTORY_*` but its own) and a home folder of its own.
- **Machine name:** a sandbox never shows this computer's name (on a device
  pairing with it, in the app's footer, in move codes): it's `qa-<name>`, or
  what `up --machine-name NAME` chooses (for a screenshot or a video, say).

## Start

From the repository (`~/factory`), always through `scripts/qa.sh`:

```sh
scripts/qa.sh up                       # {"ok":true,"name":"qa1","port":7500,…}
scripts/qa.sh --rev factory/frontend-2 up kenji   # a branch or commit instead of HEAD
```

The first run for a commit exports it with `git archive` and builds factoryd,
claktory-account and claktory-qa into the QA root. That takes a few minutes;
after that the build is reused. It builds only what's committed, never
uncommitted changes. Every commit shares one cargo build, so the script makes
sure each commit's binaries really are its own. It builds one commit at a time
and checks that every crate of the commit was compiled in that build. A commit's
binaries are reused only with the `bin/<commit>/commit` file the check writes.
A build that reused another commit's work fails with `stale_build` and keeps
nothing. The QA root is `CLAKTORY_QA_ROOT`, or `~/Library/Caches/claktory-qa` on
macOS (`~/.cache/claktory-qa` on Linux), never the temporary folder, which
macOS clears of files it thinks are unused. It holds the build (about 4 GB) and each commit's binaries
in `bin/<commit>`. Delete old ones there, or the whole root, when you're done.

Every command prints one JSON line, `{"ok":true,…}` or
`{"ok":false,"error":…,"message":…}`. It exits with one of these codes:

| Exit | Meaning |
|---|---|
| 0 | done |
| 1 | failed |
| 2 | usage |
| 3 | refused, by the sandbox or as unsafe |
| 4 | the sandbox isn't running |

`scripts/qa.sh help` lists the commands.

## Driving a sandbox

```sh
Q=scripts/qa.sh
$Q workspace qa1 account.state         # {"ok":true,"value":"CLAKTORY_ACCOUNT_STATE_SIGNED_OUT"}
$Q sign-in qa1                         # signs in as qa@example.test, reading the code from its mail
$Q send qa1 CreateProject '{"name": "Site"}' --then 'projects[name=Site].id'
$Q send qa1 CreateAgent '{"project_id": "site", "name": "Lea"}'
$Q wait qa1 'agents[name=Lea].status' AGENT_STATUS_IDLE
$Q watch qa1 --secs 20 --path 'pipes[0].state'   # what the sandbox sends, as it sends it
$Q cli qa1 status --json               # factoryd's own commands, against the sandbox
$Q transcript qa1 Lea --last 20        # an agent's entries, oldest first
$Q restart qa1 --kill                  # kill -9 its factoryd and start it again
$Q down qa1                            # or: down --all (only yours)
```

- **Whose sandboxes:** the QA root is shared by every agent on this computer,
  so each sandbox records who made it (`owner`, with `created_ms`), and `list`
  shows both.
  - The owner is `CLAKTORY_QA_OWNER` if set, else `FACTORY_AGENT_ID`, else
    the agent whose checkout `scripts/qa.sh` ran from (`…/trees/<id>/…`),
    else that checkout's path.
  - `down --all` ends only your own. Its answer names what it `ended` and what
    it `left` (someone else's, or a sandbox from before owners were recorded),
    with each one's owner in `owners`.
  - `down --all-agents` ends everyone's. Use it only when you mean it.
  - `down NAME` ends that one, whoever made it.

- **Requests:** `send` takes any request of `proto/factory/v1/factory.proto`
  by its message name, with its fields as JSON under the proto's names
  (`project_id`). Enums take their names (`"PIPE_ACTION_PAUSE"`). `requests`
  lists the request names, and `requests CreatePipeInvite` lists one request's
  fields. The answer comes back as JSON: `{"answer":{"ack":{}}}`,
  `{"answer":{"pipe_invite":{…}}}`, or the sandbox's refusal, for example
  `{"ok":false,"error":"invalid_request","message":"…"}`, which exits 3.
- **Workspace:** `workspace` prints the whole Workspace, every field including
  defaults. Give a PATH to get just one part: `account.state`,
  `agents[0].name` or `agents[name=Lea].status`. `[field=value]` picks the
  first element whose field has that value.
- **Waiting:** `wait` waits until PATH names VALUE, or names anything if you
  leave VALUE out. It waits 30 s unless you give `--secs`, then fails with
  what PATH names at that point.
- **Transcripts:** `transcript NAME AGENT` gives the agent's latest entries,
  oldest first, as the app gets them when it opens the agent (with
  `has_earlier`). `--last N` gives the last N (1000 at most), and `--from SEQ`
  every entry from that seq on. Sending `LoadEntries` yourself, give
  `before_seq`: it's where the page ends, exclusive, so the default 0 is an
  empty page.
- **Restarts:** `restart NAME` stops the sandbox's factoryd as an update
  would and starts it again, and `restart NAME --kill` kills it at once, as a
  crash would. Either way it comes back on the same port with the same
  settings, data and environment (qa-model included), and the account service
  and qa-model stay up. Restart it this way rather than by hand: a factoryd
  started without the sandbox's environment has no model, and its agents show
  "No model available". `env NAME` prints that whole environment as `export`
  lines, for factoryd's own commands.
- **Mail:** `code` gives the newest sign-in code the sandbox emailed. Its
  emails are files in the folder that `up` names as `mail`.
- **Two Claktories:** pipes and pairing between them work like this:

  ```sh
  $Q up a; $Q up b --no-account
  $Q send a CreateProject '{"name":"Site"}'; $Q send a CreateAgent '{"project_id":"site","name":"Lea"}'
  $Q send b CreateProject '{"name":"Marketing"}'; $Q send b CreateAgent '{"project_id":"marketing","name":"Max"}'
  CODE=$($Q send a CreatePipeInvite '{"project_id":"site","name":"Launch"}' | python3 -c 'import json,sys; print(json.load(sys.stdin)["answer"]["pipe_invite"]["code"])')
  $Q send b AcceptPipeInvite "{\"code\":\"$CODE\",\"project_id\":\"marketing\",\"name\":\"Launch\"}"
  $Q wait a 'pipes[0].state' PIPE_STATE_APPROVAL
  ```

- **Logs:** each sandbox's folder has `factoryd.log` and `account.log`.
- **Without an account service:** `up --no-account` starts a Claktory with
  signing in off.
- **Speech recognition:** `up --speech MODEL` starts a Claktory whose voice
  messages transcribe, with `speech_model` READY in its Workspace.
  - **MODEL** is the speech model as a Claktory keeps it: a `.gguf` with its
    `.receipt` beside it. The dev Claktory's will do (it's only read):

    ```sh
    scripts/qa.sh up --speech ~/.factory-dev/models/speech/parakeet-tdt-0.6b-v3-Q8_0.gguf
    ```

  - **The model is copied** into the sandbox's `home/models/speech`, as an
    APFS clone where it can be, with its modified time kept, since the receipt
    names it. Nothing is downloaded.
  - **claktory-speech:** the first `--speech` run for a commit builds it from
    that commit, with the same check as the other binaries. That build
    compiles transcribe.cpp (ggml) from source and needs CMake; the very first
    one takes several minutes. Later runs reuse it from `bin/<commit>`.
  - **The environment:** the sandbox's factoryd runs it through
    `FACTORY_SPEECH`, kept by `restart` and printed by `env`.
  - **Not in the container:** `--container` refuses `--speech`.
- **A release's version:** `up --version v0.1.17` starts a Claktory whose
  factoryd says it's v0.1.17, as a release's does, so an app of another
  version meets the same compatibility checks it would on that release. A
  sandbox otherwise says "dev", and those checks don't run.
  - **Only the number:** nothing else of a release is stamped in. There's no
    update channel, and the sandbox is as sealed as any other.
  - **Built once per commit and version,** into `bin/<commit>-<version>`, with
    the same check as the other binaries. Only `vX.Y.Z` is taken.
  - **Not in the container:** `--container` refuses `--version` for now.

## The person's input on Claktory's browser: `replay`

`replay NAME SCRIPT [--toolbar PX] [--png FILE]` sends a scripted list of
pointer and wheel events to the sandbox's browser, as the app's live view
sends them during the person's turn.

1. **Its connection watches** the browser (WatchAgentBrowser, `max_width`
   0: frames as the screen is) and acknowledges every frame, so the next
   can come.
2. **It takes it over** (TakeOverAgentBrowser) when an errand drives it.
   In the person's turn (BROWSING after OpenAgentBrowser, say) that changes
   nothing. It never hands it back: send HandBackAgentBrowser when done.
3. **Each event goes on time, as its own AgentBrowserInput,** on the newest
   frame's `frame_seq`.
4. **With `--png`,** the frame shown about a second after the last event is
   saved as a PNG (waiting up to 3 s more for one still coming).

A script is JSON lines, one step each:

```text
{"move": [530, 270]}                 the pointer to (x, y)
{"down": [590, 282]}                 a button down there ("button": 1 main, 2 middle, 3 secondary)
{"up": [590, 282]}                   and up
{"wheel": [900, 520], "dy": 50}      the wheel there by (dx, dy) frame pixels: 50 is one notch
{"wait_ms": 16}                      nothing for that long
```

- **Points are the page's CSS pixels,** from its top left, at a device pixel
  ratio of 1. The live view counts the browser's whole screen, Chrome's
  toolbar included, so give the toolbar's height with `--toolbar`; it's
  added to every y.
- **A wheel event of 50 frame pixels is one notch.** The host rounds to whole
  notches, at least one, at most 10. Send a burst as separate events, never
  one event of 150.
- **At most 240 events a second** reach the browser; more are dropped.
- **The live view must be on** in the sandbox's settings (`"browser":
  {"live_view": true}` in its `home/config.json`, applied at once), and its
  factoryd must run Claktory's browser; otherwise the sandbox refuses the
  watch, and `replay` exits 3 with its words.
- **Frames come at most 1280 pixels wide** (the backend's width for
  `max_width` 0), so the PNG is the frame as sent. Positions don't depend on
  it: they're fractions of the screen.

`replays/cursor-probe.jsonl` drives the cursor probe page
(`https://claktory.com/qa/fixtures/w3/cursor-probe`): 40 moves of 3 px about
16 ms apart onto Go, two presses on it held 80 ms, then three single wheel
notches 700 ms apart and three 40 ms apart. It never clicks Reset.

```sh
$Q send qa1 OpenAgentBrowser '{"url": "https://claktory.com/qa/fixtures/w3/cursor-probe"}'
$Q replay qa1 crates/claktory-qa/replays/cursor-probe.jsonl --toolbar TOOLBAR --png probe.png
```

The probe page shows the toolbar's height itself, as line G's
screenY − clientY, once any event has reached it.

## A feature's paths, end to end

1. **Plan.** Write the numbered paths from the changelog line and the diff,
   as in `qa-paths-195.md`, and mark which ones the backend can show.
2. **Start.** Run `scripts/qa.sh up` on the commit that landed it
   (`--rev <commit>`).
3. **Run.** For each drivable path, run its steps with `send`, `wait` and
   `workspace`, and keep the JSON as the evidence.
4. **Report.** Write each path as pass or fail with its JSON, and the findings
   with how to reproduce them. Then run `scripts/qa.sh down --all`.

## Agents that run: qa-model

`up NAME --model` or `up NAME --script FILE` starts qa-model beside the
sandbox. qa-model is a scripted model on this computer that speaks
OpenRouter's wire, and every agent in the sandbox, Rivet included, uses it
(`openrouter/openai/gpt-4.1-mini`, with a made-up key that opens nothing).
Its answers are deterministic.

- **Rules first:** the first rule in the script that fits the conversation's
  last message answers.
- **Built-in answers:** when no rule fits:
  - A message with lines `call TOOL {json}` calls those tools, so
    `ask qa1 Lea 'call bash {"command": "ls"}'` runs `ls`.
  - A tool's result is answered "TOOL answered: <its first line>".
  - Anything else is answered "OK: <its first line>", or the script's
    `default`.

```json
{
  "default": "Noted.",
  "rules": [
    {"agent": "Max", "match": "Untrusted message", "calls": [{"tool": "pipe_send", "args": {"text": "Noted: Friday."}}]},
    {"agent": "Max", "after": "pipe_send", "reply": "Answered them."},
    {"regex": "^deploy (\\w+)$", "once": true, "calls": [{"tool": "bash", "args": {"command": "echo deploying"}}]},
    {"after": "bash", "result": "exit 1", "reply": "That failed."}
  ]
}
```

A rule fits when all of the conditions it has hold:

| Condition | Fits when |
|---|---|
| `match` | The last message is from the person or an event, and contains this text (any case). |
| `regex` | The last message is from the person or an event, and matches this. |
| `after` | The last message is this tool's result (`"*"` for any tool). |
| `result` | That result contains this text. |
| `agent` | The agent is named this (its system prompt's "You are NAME, a persistent agent"). |
| `once` | The rule hasn't answered yet. |

`match`, `regex` and the built-in answers read a message's text inside its
`<sent …>` or `<event …>` tag. The log keeps it as the agent got it, as
`raw`. A rule with no conditions fits anything. It answers with `reply` (text),
`calls` (tool calls, each `{"tool", "args"}`) or both.

- **Changing the script:** qa-model reads it again for every request, so
  `script NAME FILE` (which checks it first) changes the answers between
  steps. A script with a mistake gets the agent an error that says what's
  wrong.
- **Talking to an agent:** `ask NAME AGENT TEXT` messages the agent as the
  person and waits until it's done. It prints the entries its run added
  (your message, tool calls with their output, events, replies) and its last
  reply.
- **Seeing what it was asked:** `model NAME [--agent A] [--last N]` shows
  every request qa-model got, with the last message, the tools offered, the
  rule that fit and the answer. `--system` adds the system prompt, so you can
  check what an agent was told (a pipe's "# Pipes" section, say). The full log
  is the `model_log` file that `up` names.

- **OpenRouter's model list:** qa-model also serves the list factoryd
  fetches from OpenRouter (Settings → Providers' **Refresh models**, and the
  monitor that reads it every few hours), so no real OpenRouter is needed.
  Without a file it's a built-in list of 56 models (`qa/model-01` … `qa/model-55`
  and the sandbox's own `openai/gpt-4.1-mini`). To change it, write
  `openrouter-models.json` in the sandbox's folder (`up` names it as
  `openrouter_models`) as OpenRouter answers, `{"data": [{"id", "name",
  "context_length", "architecture": {"output_modalities": ["text"]},
  "supported_parameters": ["tools"]}, …]}`; it's read again at every fetch,
  so add or remove a model between two refreshes. factoryd keeps a list
  only when it has 50 models agents can run on (taking tools, answering in
  text), so keep at least that many, or remove some below 50 to see a broken
  list kept out. Needs a factoryd built by `scripts/qa.sh` (the QA build),
  from a commit that has this.

**Careful:** the tools a script calls run for real, as you, on this computer
(`bash` included), so keep the commands harmless, or run the sandbox in a
container (next section). qa-model itself reaches nothing.

## A Claktory that lies about whose it is: `--claim-email`, `--claim-claktory-id`

For Fugo's hostile pass (E11 row 6): `up NAME --claim-email ana@example.com`
starts a sandbox whose Workspace says it's signed in to that account, and
`--claim-claktory-id ID` makes its direct-message leaf offer name another
Claktory (one of that account's listed ones, by its id), and so its leaf id.
Give either alone, or both. It's signed in to its own account all the same;
only what it says lies, so the person's device must catch it by checking
claktory.com's list and the endpoint it dials. A QA build of factoryd only
(`qa-lies`, which a release build refuses: `scripts/check-qa-lies.sh`), and a
real Claktory's folder or port refuses to start with either set.

```sh
scripts/qa.sh up bo --claim-email ana@example.com --claim-claktory-id <Ana's Claktory id>
scripts/qa.sh list                          # its "claims"
```

## Claude at its usage limit: `--claude`

`up NAME --claude` signs the sandbox in to Claude, fake, on qa-model, so you
can see what a Claude subscription does at its usage limit: an agent waits
for the reset, Try now asks again, and it carries on once there's room
(#332, friend bug #3). Every agent, Rivet included, runs on
`anthropic/claude-haiku-4-5`, answered from the script as above (with
`--script FILE`, or the built-in answers).

```sh
Q=scripts/qa.sh
$Q up qa1 --claude --no-account
$Q send qa1 CreateProject '{"name": "Site"}'; $Q send qa1 CreateAgent '{"project_id": "site", "name": "Lea"}'
$Q ask qa1 Lea 'Hello'                      # "OK: Hello", on Claude's wire
$Q limit qa1 --for 600                      # at its limit, resetting in 10 minutes
$Q send qa1 SendMessage '{"agent_id": "lea", "text": "Still there?"}'
$Q workspace qa1 'agents[name=Lea].usage_wait'           # why Lea waits
$Q workspace qa1 'agents[name=Lea].usage_wait_until_ms'  # until when
$Q limit qa1 --off                          # room again, as after a plan upgrade
$Q send qa1 TryAgainNow '{"agent_id": "lea"}'   # or Try now in the app: Lea answers
$Q limit qa1 --for 20                       # or let it reset by itself: Lea carries on
```

- **The limit:** `limit NAME --for SECS` puts the account at its usage limit
  until SECS from now. Every model request is refused as Anthropic refuses
  one at the limit (a 429 that says when it resets), and the account's usage
  reads its five-hour window as full until then. At that time it answers
  again by itself. `limit NAME --off` lifts it at once, as upgrading the plan
  would, but Claktory only notices when it asks again: at Try now, when you
  message the agent, or at its usage read each minute while an agent waits.
- **The account:** Claude Max 5x, `claude@example.test`, as Settings →
  Providers shows it. `up` prints it as `claude`, with `limited_until_ms`.
- **Reading full while it answers:** `limit NAME --for SECS --reads-full`
  makes the account's usage read its weekly window at 100% until then while
  every message is still answered, as Anthropic answered Matt's account at a
  weekly 100% (Bruno 52170). Claktory sends requests to another account with
  room first while one reads full, and back once it resets.
- **Two accounts:** `up NAME --claude --claude-accounts 2` signs in a second
  fake account after the first, `claude2@example.test`, with its own limit
  (`limit NAME --account 2 …`). `up` prints it under `claude.second`. Each
  answered message's line in qa-model's log (`model NAME`) says which account
  it came on (`account`). For example, an account read full gives way to the
  one with room, and every account read full still answers on the first:

  ```sh
  $Q up qa2 --claude --claude-accounts 2 --no-account
  $Q send qa2 CreateProject '{"name": "Site"}'; $Q send qa2 CreateAgent '{"project_id": "site", "name": "Lea"}'
  $Q limit qa2 --for 600 --reads-full          # the first reads weekly 100%, still answering
  $Q restart qa2                               # reads every account's usage now (else at its next read)
  $Q ask qa2 Lea 'Hello'
  $Q model qa2 --last 1                        # "account": "claude2@example.test"
  $Q limit qa2 --account 2 --for 600 --reads-full; $Q restart qa2
  $Q ask qa2 Lea 'Again'                       # both read full: back on claude@example.test
  ```
- **Images as files (#382):** as with Anthropic, an image an agent reads
  goes to Claude once, uploaded to qa-model's Files API, and later requests
  name its file instead of sending it again. Uploads are kept in the
  sandbox's `claude-files/`. qa-model's log has a line for each upload
  (`uploaded`), each delete (`deleted`, at compaction or /clear) and each
  message with images (`image_files`, `inline_images`, `missing`). Delete a
  file there to have qa-model lose it, as Anthropic lets one go: the next
  message naming it is refused (404 "File not found"), and Claktory sends
  that request again with the images inline. Images read before an update to
  #382, or with no digest, always go inline.
- **What it is:** qa-model answers Anthropic's wire too (messages, usage,
  profile, the sign-in's token), and the sandbox's `auth.json` holds a sign-in
  with fixed, obviously fake tokens that only qa-model accepts. factoryd
  reaches it through `FACTORY_QA_ANTHROPIC_URL` (in `env NAME`).
- **Only a QA build:** that variable is read only by a factoryd built with
  the `qa-provider` feature, which `scripts/qa.sh` builds sandboxes with. A
  release build can't have it (it doesn't compile), and even a QA build takes
  only an address on this computer, never with the real Claktories' homes or
  ports, and won't start otherwise. A factoryd without it is refused by `up
  --claude` before anything starts. Commits from before it have no
  `--claude`, and neither does `--container`.
- **Seeing it in the app:** Claktory Dev built against the sandbox's port
  (as in the integration tests QA runs on the Mac and simulators) shows the
  wait beside the agent, with Try now.

## In a container

`scripts/qa.sh --container COMMAND…` runs the same commands inside a Linux
container (podman, which must be running). claktory-qa, its sandboxes and
their agents' tools all run there, with no network at all, so a script's tools
can do whatever a test needs without touching this computer. Arjun's
`scripts/qa.sh` sets it up:

- **Builds:** the first run for a commit builds static Linux binaries and an
  image from them.
- **The container:** it's named `claktory-qa` and stays up between commands.
  Switching commits replaces it, and its sandboxes go with it.
  `podman rm -f claktory-qa` removes it.
- **Files:** a file you give `up --script` or `script` is copied in for you.
- **Container paths:** the paths the JSON prints (logs, mail, the model's
  log) are inside the container. Copy one out with
  `scripts/qa.sh --container pull PATH [DEST]` (to the current folder unless
  you give DEST), or use `model`, `code` and `workspace`, which work the
  same.

```sh
scripts/qa.sh --container up --no-account --model
scripts/qa.sh --container ask qa1 Rivet 'call bash {"command": "uname -a"}'
scripts/qa.sh --container down --all
```

What a sandbox can't show: the app's screens (they need Claktory Dev or a
simulator), and how a real model would behave.
