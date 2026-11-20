# Working on Claktory

Claktory may be running from this repo while you change it, and you may be one of its agents. Never disturb the running Claktory.

The product is Claktory (claktory.com); the code keeps its old name, Factory, inside, and so do these instructions where they name things: `factoryd`, the crates, `~/.factory` and `~/.factory-dev`, `Factory.app` and `Factory Dev.app`, bundle ids, LaunchAgent labels, ports, config keys, the protocol, extension ids such as `memory:factory`, and branches such as `factory/<id>`. Don't rename those; write "Claktory" in anything people or agents read.

The person you work with uses the real Claktory: that's where they talk to you and give feedback. The dev Claktory is yours: start it, restart it and try your changes in it as much as you need.

## Real and dev Claktory

| | Real Claktory | Dev Claktory |
|---|---|---|
| Backend | `~/.factory/bin/factoryd` under its supervisor, `~/.factory/bin/Claktory Agents.app` (`factoryd-supervisor` in installs before it), port 7420 | `~/.factory-dev/bin/factoryd` under `target/debug/factoryd --dev supervise`, port 7421 |
| Data | `~/.factory` | `~/.factory-dev` |
| App | Claktory (`~/Applications/Factory.app`) | Claktory Dev (`app/build/Factory Dev.app`) |

- Ship a change to the person with `scripts/release.sh`. It builds a new version into `~/.factory/releases` without touching the running Claktory; their app then shows an Update banner and they choose when. `scripts/release.sh --dev` does the same for the dev Claktory. Updating is safe even while you're working: agents resume after the restart, app-only releases don't restart the backend, and a backend that doesn't start is rolled back.
- Every change the person can notice adds a line under `## Unreleased` in `docs/CHANGELOG.md`; Claktory shows it in the app's What's new. Write it for the person, the way the app is written (rules under "Changelog style" in `~/.factory/workspace/coordination/ux-writing.md`):
  - What changed for them and what it means, never how it's built (no crates, fields, file or test names).
  - `- Added: …`, `- Changed: …` or `- Fixed: …`, then one or two short sentences that read on their own, benefit first: "- Added: Agents can show you files beside their conversation."
  - "You", the app's own words (Claktory, device, connect, sign in) and controls named as they read on screen.
  - One line per change, not per commit: extend the existing line for a follow-up. Add it only once it works on main.

  A release of the real Claktory needs those entries stamped with its version first (`scripts/changelog.sh stamp`, then commit); `scripts/release.sh` builds that version and refuses when there's nothing new. Versions are v0.1.0, v0.1.1, v0.1.2…: every release counts up the last number. Stamp with `--incompatible` (v0.1.7 → v0.2.0) only when this version's app can't work with an older backend, or its backend with an older app (a protocol change an older app can't follow, say), never for features or fixes; it's a deliberate choice. Versions before v0.1.0 were date stamps (2026.11.19-1958), older than every numbered one. `--dev` releases need none of this. Changes with nothing the person can notice (behaviour-preserving refactors, test-only changes) need no line.
- A release of the real Claktory is always signed with the Developer ID, the hardened runtime and Apple's notarization (stapled to the app), by `scripts/sign-release.sh`. It needs a "Developer ID Application" identity and the App Store Connect API key (the same `~/.appstoreconnect/claktory.env` and .p8 as TestFlight, or the vault's secrets). Without them `scripts/release.sh` refuses to build it: an app signed ad hoc is another app to macOS, so it can't read what the person's Claktory keeps in the Keychain (its connections to other Claktories, say), and the person's Mac would look unpaired. `--adhoc` overrides that, only on purpose and never for the person's Claktory. `--dev` releases are always ad hoc. `scripts/sign-release.sh DIR --dry-run` signs a copy of a release folder with the Developer ID and checks it without sending anything to Apple; never run it on a published release.
- `scripts/publish.sh` publishes a release to GitHub, `claktor/claktory` (`--repo OWNER/NAME` for another), for Claktories installed elsewhere with the online installer (`scripts/install-online.sh`, uploaded as `install.sh`); they download it within half an hour and offer it the same way. It also makes the source public: from `main`, committed and clean only, it pushes main to the repository before uploading (`scripts/push-source.sh`; never forced, except once over a new repository's README-only first commit), after `scripts/check-secrets.sh` (gitleaks: `brew install gitleaks`) finds no secret anywhere in the history: keys, tokens, private keys, credential and database files, paths in a real home folder, this machine's names. Any finding stops it; an allowed false positive (a test fixture's fake key) is a narrow, explained exception in `.gitleaks.toml`, committed. Never commit anything private, even to delete it later: the whole history goes public. Leave publishing to Coordinator and the person. `scripts/publish.sh --local DIR --dev` publishes to a channel in a folder instead, to try it out: start the dev backend with `FACTORY_UPDATE_URL=file://DIR/latest.json scripts/dev.sh backend`. It also builds the backend for Linux (x86_64 and aarch64, static) in a container with `scripts/build-linux.sh`, which needs podman or docker running; `--no-linux` leaves Linux out. On Linux the installer runs the release's `scripts/install-linux.sh` (a systemd user service).
- The backend runs under `factoryd supervise` (in the real Claktory, `~/.factory/bin/factoryd-supervisor`, run by the LaunchAgent). Never stop or replace the supervisor; it's what undoes a bad update.
- Try changes in the dev Claktory: `scripts/dev.sh` rebuilds and restarts the dev backend and the Claktory Dev app. Use `scripts/dev.sh backend` or `scripts/dev.sh app` to restart only one. The dev backend logs to `~/.factory-dev/factoryd.log`.
- Claktory Dev opens in the background and never takes focus. Don't open it any other way.
- Never open, quit or relaunch the real app, Claktory (`Factory.app`). Don't touch `~/.factory`, port 7420, `~/Applications/Factory.app` or the `dev.factory.factoryd` LaunchAgent.
- Don't run `scripts/install.sh` unless you're asked to. It replaces the real Claktory and restarts it, which interrupts every agent, you included.
- Never make a plain `git worktree`, clone or copy of this repository to work in. Your own checkout is Claktory's Own checkout extension (`tree`, on unless turned off for you): a copy-on-write checkout of the files git tracks (build output starts empty), on your branch `factory/<your id>` (`factory-dev/<id>` for dev agents); commit there, rebase onto main and land with `tree_land`. For a throwaway snapshot to check something, export what's committed: `git archive HEAD | tar -x -C <dir>` (never `cp -R` the main tree: it copies `target/`, hundreds of GB).
- The dev Claktory has its own sign-ins, made in Claktory Dev under Settings → Providers.
- The two apps have different icons: Claktory is a white cog with a blue shadow on graphite; Claktory Dev is the inverse, light, labeled DEV, and its window has a faint "DEV VERSION" across it. `swift scripts/icons.swift` redraws both icons.

## Product rules

- Everything a user can do must be possible in the app, as easily as possible. Never make the command line or editing a file the user's only way.
- All state lives in the backend (tenet 7). There are three exceptions, kept on the device: the window's size and position, because each device has its own screens; unsent drafts in the agents' message boxes, by the person's decision; and which Claktory the app shows (its pairing code: the backend's address and the device's key, and those of the others it has connected to), since a device can't ask a backend where its backend is, also by the person's decision. Add no other device-local state unless the person explicitly decides it.
- The app shows the real state, never placeholders. For example, an agent whose model can't run shows "No model available", not the model's name.
- Prompt caching: providers cache a conversation by its prefix, so what changes during a conversation is only ever appended to its end. An agent's system prompt and tools are written when its context starts (first run, compaction, /clear), kept with the segment (its frame) and sent byte for byte; nothing earlier in the conversation is ever changed. The exception is the person changing the agent itself, which applies from its next model call, mid-run too: a new model gets a new frame, tools and all; new instructions or a new name a new system prompt with the frame's tools kept; a new project or folder a new system prompt and tools, since they work somewhere else; a new effort goes into the next request as it is. The agent is told in a short Claktory event ("Your model is now …", "Your instructions changed; the new ones are in your system prompt from now on."). Everything else that changes meanwhile (whom it can message, the models available, its extensions) reaches it only as a Claktory event before its next step, left out of what compaction summarizes, and its system prompt and tools follow at the next context. An extension turned on mid-context is usable from the next call all the same, with the frame untouched: every frame has the small tool `extension_call {tool, args}`, which runs any tool of the agent's extensions that are on now, and the event carries the extension's instructions and its tools' names, descriptions and parameters. An extension turned off has its tools refused at once.
- What an effort change costs the cache, measured in the dev Claktory: on Anthropic models with adaptive thinking (`output_config.effort`: Sonnet 4.6, Opus 4.6 and later) the next request misses the whole cache, system and tools included, and writes it all again once; going back to the earlier effort reads that effort's cache again while it lives. On Anthropic models with a thinking budget (measured on Sonnet 4.5; Haiku 4.5 and Opus 4.5 send the same) system and tools stay cached and only the messages are written again. OpenAI and Codex document their cache as a match on the input's prefix, which the reasoning effort isn't part of (not measured: dev has no sign-in for them); OpenRouter leaves it to the model's own provider. So an effort change is cheap on some models and costs one full cache write on others: fine for the person's occasional change, never something Claktory should do on its own.

## Layout

- `crates/factoryd`: the backend. It owns all state: settings (`config.json`), agents and their transcripts.
- `crates/factory-agent`: the coding agent: model providers, sign-in, tools, compaction.
- `crates/factory-protocol`: Rust types generated from the protocol schema.
- `crates/claktory-account`: the account service (claktory.com's sign-in, the vault's service share, check-ins), a server of its own; its HTTP API, the contract with factoryd, is `crates/claktory-account/API.md`. `scripts/deploy-account.sh HOST` installs or updates it on a Linux server over ssh, with Caddy in front (steps in `crates/claktory-account/DEPLOY.md`); `scripts/test-deploy-account.sh` tries that in a podman container. Deploying it is DevOps's.
- `proto/factory/v1/factory.proto`: the wire protocol, the contract between backend and app. Only add fields and messages; never renumber or reuse them.
- `app/`: the Flutter app. It only displays the backend's state and sends requests.

## Checks

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
cd app && flutter analyze && flutter test
```

Each crate's integration tests are one test binary, `tests/all/` (each file there is a module), so they link and start once. While you work, run only what your change touches:

```sh
cargo test -p factoryd --test all monitors::          # one file's tests (tests/all/monitors.rs)
cargo test -p factoryd --test all monitors::checks    # tests whose names start so
cargo test -p factory-agent --test all bash::
cargo test -p factoryd --lib                          # unit tests in src/
```

A new test file goes in `tests/all/` with a `mod` line in `tests/all/main.rs`; shared helpers are `tests/common` (factoryd) and `tests/support` (factory-agent). Only a test that needs a process of its own (process-wide state, such as `tests/tls.rs`) is a file of its own in `tests/`.

After changing `proto/`, run `scripts/gen-proto.sh` to regenerate the Dart types. The Rust types regenerate on every build.

The iPhone app (`app/ios`, Xcode needed): `scripts/ios.sh --dev --run` builds Claktory Dev for the Simulator and opens it there, without opening Simulator's window; `xcrun simctl io booted screenshot <file>` shows what it looks like. It pairs with a Claktory only through the private tunnel. `app/integration_test/phone_test.dart` drives it end to end (pair, open an agent, message it, wait for the reply); its header says how to run it. Its native code (`app/ios/Runner`, such as reading QR codes) has tests in `app/ios/RunnerTests`: after a build, `cd app/ios && xcodebuild test -quiet -workspace Runner.xcworkspace -scheme Runner -destination 'platform=iOS Simulator,name=iPhone 16 Pro' -only-testing:RunnerTests`.

The Mac app's native store for its connections (`app/macos/Runner/ConnectionStore.swift`: the Keychain, user defaults when ad hoc) has a check that runs without opening the app; `app/macos/RunnerTests/ConnectionStoreCheck.swift`'s header says how. Never run the macOS RunnerTests through Xcode: they launch the app under the real Claktory's bundle id.

`scripts/testflight.sh` builds the iPhone app (com.claktory.app; Claktory Dev is com.claktory.app.dev) for the App Store, signs it automatically and uploads it to App Store Connect for TestFlight; `--no-upload` only builds the signed .ipa into `app/build/ios/testflight/`. It needs the team ID and an App Store Connect API key, from the environment or `~/.appstoreconnect/claktory.env` (its header lists them), and refuses without them. Uploading is the person's call, through Coordinator, like releases.

## The manual

`docs/MANUAL.md` is built into every agent's system prompt; it's how Rivet, the agent every Claktory starts with, knows Claktory. Keep it true: when you change what Claktory can do or how the app works, change the manual in the same change. `{{home}}`, `{{config}}` and `{{workspace}}` in it are replaced with the real paths, and `{{user}}` and `{{User}}` with the user's name (or "the person" and "The person").

Every agent gets only its core. A line `<!-- admin: <slug> | <summary> -->` makes what follows it (a heading's section, or one list item) a note of the Claktory manual instead: left out of the core, listed by its summary for admin agents such as Rivet (and any agent given the Claktory manual), and opened with memory_read when needed. Keep in the core what every agent needs to work (agents, events, messaging, tasks, projects); put app tours, option references and how-tos in notes, each with a unique slug and a one-line summary.

## Read before changing things

- `TENETS.md`: what Claktory is. Changes must hold to it.
- `docs/IDENTITY.md`: how the app looks and moves.
