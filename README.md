# Claktory

Claktory is where your agents live and work: persistent coding agents, each with its own name, role, prompt, model and folder, running on your own Mac or server, working together on your projects while you're away. Its backend, Factory (`factoryd`), holds all the state; the desktop app shows it.

Install the latest release on a Mac (or a Linux server, for the backend):

```sh
curl -fsSL https://github.com/claktor/claktory/releases/latest/download/install.sh | bash
```

The source is available under the PolyForm Shield License 1.0.0 ([LICENSE.md](LICENSE.md)): read it, run it, change it, but not to compete with it.

## Working on it

[TENETS.md](TENETS.md) says what Factory is. [docs/IDENTITY.md](docs/IDENTITY.md) says how it looks. [AGENTS.md](AGENTS.md) is how to work on it, for people and agents.

## Layout

- `crates/factoryd`: the backend (Rust). Listens on `ws://127.0.0.1:7420`.
- `crates/factory-agent`: the coding agent that runs Factory's agents.
- `crates/factory-protocol`: Rust types generated from the protocol schema.
- `proto/`: the wire protocol schema, the source of truth for both sides.
- `app/`: the desktop frontend (Flutter). macOS for now; Linux and Windows build from the same code later.

## Install from source

```sh
scripts/install.sh            # builds and installs Factory, starts the backend
```

Open Factory from `~/Applications` and sign in to a model provider under Settings → Providers.

Settings live in `~/.factory/config.json`, and projects in `~/.factory/workspace`. The app and your agents both change them; changes apply within a second.

## Release

```sh
scripts/changelog.sh stamp    # names the Unreleased changes with the next version (v0.1.N); commit it
scripts/release.sh            # builds that version; Claktory's app offers the update
```

## Develop

```sh
scripts/dev.sh                # dev backend (port 7421, ~/.factory-dev) + the "Factory Dev" app
```

The dev Factory runs next to the real one without touching it.

## Change the protocol

Edit `proto/factory/v1/factory.proto`, then run `scripts/gen-proto.sh` to regenerate the Dart types. The Rust types regenerate on every build.

## Test

```sh
cargo test --workspace
cd app && flutter test
```
