# Claktory

Claktory is where your agents live and work: persistent coding agents, each with its own name, role, prompt, model and folder, running on your own Mac or server, working together on your projects while you're away. Its backend, `factoryd`, holds all the state; the desktop app shows it.

Install the latest release on a Mac (or a Linux server, for the backend):

```sh
curl -fsSL https://github.com/claktor/claktory/releases/latest/download/install.sh | bash
```

### On a Linux server

Claktory also runs on Linux (x86_64 or aarch64, with systemd; NixOS too). Run the same command as yourself, not as root. It installs Claktory, keeps it running from boot, and shows a code to connect your devices: scan it with Claktory on your iPhone, or on your Mac click Add a Claktory (+) and paste it. If it can't keep Claktory running after you log out by itself, it says what to run (`sudo loginctl enable-linger $USER`).

To connect another device later, run `~/.factory/bin/factoryd pair "Device name"` on the Linux machine.

On NixOS, add `git` to your packages (your agents need it), and set `users.users.<you>.linger = true;` so Claktory keeps running after you log out.

The source is available under the PolyForm Shield License 1.0.0 ([LICENSE.md](LICENSE.md)): read it, run it, change it, but not to compete with it.

## Working on it

[TENETS.md](TENETS.md) says what Claktory is. [docs/IDENTITY.md](docs/IDENTITY.md) says how it looks. [AGENTS.md](AGENTS.md) is how to work on it, for people and agents.

The product is Claktory; the code keeps its old name, Factory, inside: `factoryd`, the crates, `~/.factory`, `Factory.app`, the protocol and the settings keys.

## Layout

- `crates/factoryd`: the backend (Rust). Listens on `ws://127.0.0.1:7420`.
- `crates/factory-agent`: the coding agent that runs Claktory's agents.
- `crates/factory-protocol`: Rust types generated from the protocol schema.
- `proto/`: the wire protocol schema, the source of truth for both sides.
- `app/`: the desktop frontend (Flutter). macOS for now; Linux and Windows build from the same code later.

## Install from source

```sh
scripts/install.sh            # builds and installs Claktory, starts the backend
```

Open Claktory (`~/Applications/Factory.app`) and sign in to a model provider under Settings → Providers.

Settings live in `~/.factory/config.json`, and projects in `~/.factory/workspace`. The app and your agents both change them; changes apply within a second.

## Release

```sh
scripts/changelog.sh stamp    # names the Unreleased changes with the next version (v0.1.N); commit it
scripts/release.sh            # builds that version; Claktory's app offers the update
```

## Develop

```sh
scripts/dev.sh                # dev backend (port 7421, ~/.factory-dev) + the Claktory Dev app
```

The dev Claktory runs next to the real one without touching it.

## Change the protocol

Edit `proto/factory/v1/factory.proto`, then run `scripts/gen-proto.sh` to regenerate the Dart types. The Rust types regenerate on every build.

## Test

```sh
cargo test --workspace
cd app && flutter test
```
