# Moving a Claktory: the bundle format

`factoryd export` writes a Claktory into one encrypted file, a **bundle**
(`*.claktory`), and `factoryd import` makes a Claktory from it on another
computer. This is the contract between the two (#217, phase 1): export is
`crates/factoryd/src/move_export.rs`, the format itself (writing, reading,
unpacking) is `crates/factoryd/src/move_bundle.rs`, and import is
`crates/factoryd/src/move_import.rs`. The plan and its reasons are Research's
`moveclaktory.md`.

Format version: **1**. A reader refuses a bundle whose `format` it doesn't
know, saying which version of Claktory made it.

## The file

```
"CLAKMOVE"            8 bytes, the magic
header length         u32, big-endian
header                that many bytes of JSON, in the clear
chunks                the encrypted stream, to the end of the file
```

The header says how to open the rest, and nothing about what's in it:

```json
{
  "format": 1,
  "kdf": {"name": "argon2id", "m": 65536, "t": 3, "p": 1, "salt": "<32 hex: 16 bytes>"},
  "cipher": "xchacha20poly1305-stream",
  "nonce": "<38 hex: 19 bytes>",
  "chunk": 1048576,
  "compression": "zstd"
}
```

### Passphrase and key

- Export makes a new **passphrase** for every bundle: 160 random bits written
  as 32 letters and digits in groups of four (`K7QM-2XPA-…`), like the vault's
  recovery key, and prints it once. It's never stored anywhere.
- Typed back, dashes, spaces and case don't matter, and `0`, `1` and `8` read
  as the letters `O`, `I` and `B` (`vault::crypto::parse_recovery`).
- The key is Argon2id (version 0x13) of the passphrase's 20 bytes, with the
  header's salt and costs, 32 bytes long. 64 MiB is all the memory opening a
  bundle needs.

### Encryption

- XChaCha20-Poly1305 in the STREAM construction (Hoang, Reyhanitabar, Rogaway
  and Vizár, "Online Authenticated-Encryption and its Nonce-Reuse
  Misuse-Resistance"): the plaintext is cut into chunks of `chunk` bytes, the
  last one possibly shorter, or empty.
- Chunk *i* (from 0) is sealed with the nonce `nonce ‖ i as u32 big-endian ‖
  last`, where `last` is one byte: 1 for the final chunk, 0 otherwise. Its
  associated data is everything before the chunks (magic, length and header),
  so the header can't be changed either.
- On disk each chunk is its ciphertext and 16-byte tag: `chunk + 16` bytes,
  except the final one, which may be shorter (down to 16 bytes, for an empty
  chunk) or whole.
- So a changed, reordered, cut-short or extended bundle doesn't open: a reader
  that reaches the end of the file must have just opened a chunk sealed as the
  last, and one that opened a last chunk must be at the end of the file.

### Inside: records

The decrypted stream is zstd (one or more frames). Decompressed, it's a
sequence of records:

```
header length    u32, big-endian
header           JSON
content          `size` bytes (files only)
sha256           32 bytes, the content's SHA-256 (files only)
```

| `kind` | Header fields | What it is |
|---|---|---|
| `file` | `path`, `size`, `mode`, `mtime_ms` | A file: its content and hash follow. |
| `dir` | `path`, `mode` | A folder, even an empty one. |
| `symlink` | `path`, `target` | A symbolic link, as it was (not followed). |
| `end` | `files`, `bytes` | The last record: how many files and content bytes came before it. |

- `path` is relative, `/`-separated, with no `.`, `..` or empty parts. A
  reader refuses anything else, and a symlink whose `target` is absolute or
  leaves the bundle's tree.
- `mode` is the Unix permission bits (`0o600` for keys: keep them).
- The first record is always the file `manifest.json`.
- The hash after each file is checked as it's written; a file whose hash
  doesn't match fails the whole unpack.

## Where things are in the bundle

| Path | What |
|---|---|
| `manifest.json` | The manifest (below). |
| `home/…` | The source's home (`~/.factory`), at the same relative paths. |
| `folders/<n>/…` | Project folder `n` of the manifest's `folders`. |
| `checkouts/<agent>/<name>/…` | An agent's own checkout's uncommitted work. |

### `home/`

Everything in the home except what's regenerable or belongs to this machine:

- **Left out:**
  - `bin/`, `releases/` and `models/`: the target installs its own platform's, and downloads the speech model again;
  - `trees/`: agents' checkouts are made again from their branches, with `checkouts/` below;
  - `trash/`, `factoryd.log*`, `factoryd.pid`, `.removing-*`, and uploads' `.partial/`;
  - `moved.json`, `moved/` and `move-epoch` (below);
  - every project folder's subtree, wherever it is (they're in `folders/`).
- **SQLite databases:** any file that starts with `SQLite format 3\0` (`factory.db`, `vault/vault.db`, agents' `database.sqlite`, apps' data) is a consistent snapshot taken with SQLite's online backup. Their `-wal`, `-shm` and `-journal` files are left out.
- **Included**, so a reader can rely on them when the source had them:
  - `config.json`;
  - `factory.db`;
  - `auth.json`;
  - `vault/vault.db` and `vault/auto.key`;
  - `account/` (`account.json`, `claktory.key`, `share.key`);
  - `remote/` (`iroh.key`, `devices.json`, `local-pairing`, `remote-port`);
  - `agents/**`, conversations, uploads and task logs;
  - `chats/**`, `apps/**`, `pipes/**` and `workspace/**`.
- Inside `workspace/` and plain folders, folders named `target` or
  `node_modules`, and `.DS_Store` files, are left out.

### `folders/<n>/`

One per distinct folder the Claktory's projects use (each project's path and
its folders' paths), as the manifest lists them. A folder inside a git
repository is that repository's top folder; one inside another listed folder
is part of it, not listed again.

- **`kind: "git"`:**
  - `repo.bundle`: `git bundle create --all`, every ref: branches (agents' `factory/<agent>` included), tags, remote-tracking refs. It's absent for a repository with no commits yet.
  - `changes.patch`: `git diff --binary HEAD`, the uncommitted changes to tracked files, staged or not. It's absent when there are none.
  - `untracked/…`: files git doesn't track and doesn't ignore (`git ls-files --others --exclude-standard`), at their paths in the repository.
  - Ignored files (build output, dependencies) aren't moved.
  - The working tree's `.git` holds only what the bundle carries: hooks, the local config, stashes' reflogs and the like stay behind. Remotes are in the manifest.
- **`kind: "plain"`:** `files/…`, every file, with the excludes above.

### `checkouts/<agent>/<name>/`

An agent's own checkout (`<home>/trees/<agent>/<name>`), whose branch is in
its repository's `repo.bundle`:

- `changes.patch`: `git diff --binary HEAD` in the checkout, absent when clean;
- `untracked/…`: as for folders.

## The manifest

```json
{
  "format": 1,
  "export_id": "<32 hex>",
  "created_ms": 1763735000000,
  "final": true,
  "version": "v0.1.14",
  "epoch": 1,
  "source": {
    "machine": "Matt's MacBook Pro",
    "os": "macos",
    "arch": "aarch64",
    "user": "me",
    "user_home": "/Users/me",
    "home": "/Users/me/.factory",
    "dev": false
  },
  "files": 18234,
  "bytes": 812345678,
  "requires": {"disk_bytes": 900000000, "memory_bytes": 268435456},
  "folders": [
    {
      "path": "/Users/me/factory",
      "kind": "git",
      "projects": ["factory"],
      "branch": "main",
      "head": "<40 hex>",
      "remotes": [{"name": "origin", "url": "git@github.com:claktor/claktory.git"}],
      "bundle": "folders/0/repo.bundle",
      "patch": "folders/0/changes.patch",
      "untracked": "folders/0/untracked",
      "files": null,
      "bytes": 81234567
    }
  ],
  "skipped": [{"path": "/Volumes/shared/media", "why": "left out (--skip)"}],
  "checkouts": [
    {
      "agent": "builder",
      "name": "factory",
      "tree": "/Users/me/.factory/trees/builder/factory",
      "repo": "/Users/me/factory",
      "branch": "factory/builder",
      "head": "<40 hex>",
      "patch": "checkouts/builder/factory/changes.patch",
      "untracked": null
    }
  ],
  "tasks": [{"agent": "builder", "task": "task-12", "command": "cargo test", "started_ms": 1763734000000}]
}
```

- **Identity:**
  - `export_id` is unique to this bundle. Import records it (the `moves` table). Nothing stops the same bundle being imported on two computers, which would make two Claktories with one identity: import says so at the end, and a bundle without `final` (its source still runs) is refused unless `--same-identity` is given. A fresh identity for such an import is the better answer, for later. The account service's rotation (`POST /v1/move`) lets only the first moved copy keep the account.
  - `final` says the source was tombstoned after writing it (below).
  - `version` is the source's factoryd version. Import refuses when it's newer than its own.
- **`epoch`:** the move epoch. The source's `<home>/move-epoch` (a decimal number; 0 when absent), plus one. Import writes it to the target's `<home>/move-epoch`.
- **`source`:**
  - `machine` is the source's name, as devices show it;
  - `os` and `arch` are Rust's `std::env::consts` values (`macos`, `linux`; `aarch64`, `x86_64`);
  - `home` is the source's home folder, and `user_home` its user's;
  - `dev` is set for a dev Claktory.
- **Sizes:** `files` and `bytes` count the content to unpack, as planned when the export started.
- **`requires`:**
  - `disk_bytes` is what unpacking and setting it up needs free on the target: the content, plus room for git to check the folders out.
  - `memory_bytes` is the memory it needs.
- **`folders`:**
  - `path` is the source's absolute path; `projects` are the ids of the projects that use it.
  - For git: `branch` is the checked-out branch (empty for a detached HEAD), `head` the commit, and `remotes` each remote's name and fetch URL.
  - `bundle`, `patch`, `untracked` and `files` are bundle paths, or `null` when absent.
  - `bytes` is its content in the bundle.
- **`skipped`:** folders the projects use that aren't in the bundle, and why: `--skip`, missing, or one that holds Claktory's home.
- **`checkouts`:** each agent's own checkout, from its `<name>.json`. `tree` and `repo` are source paths, and `head` is the checkout's commit.
- **`tasks`:** background tasks still running when the bundle was made. Processes don't move: on the target they're over, and the agents are told.

**Re-pathing** is import's: every absolute source path in the manifest and in
`home/config.json` maps by prefix, longest first. By default `source.home` maps
to the target's home, and `source.user_home` to the target's user home. Agents'
conversations and notes are history, and stay as they are.

## The source after `--final`: the tombstone

`factoryd export --final` freezes the Claktory, writes the bundle, reads it
back whole, and only then tombstones the source. If anything fails before
that, the source starts again as it was, and the bundle is deleted.

The tombstone:

1. **`<home>/moved.json`:**

   ```json
   {"format": 1, "export_id": "<32 hex>", "at_ms": 1763735100000, "epoch": 1,
    "machine": "Matt's MacBook Pro", "to": "", "to_os": "", "version": "v0.1.14",
    "bundle": "/Users/me/claktory-….claktory"}
   ```

   - `machine` is this computer's name.
   - `to` is where it moved: empty after a bundle (`--final`); after a move over the network, the target's machine name, with `to_os` its system (`linux`, `macos`).
   - A backend that finds `moved.json` runs no agents and starts no endpoint (moved mode, import's side). It answers every request with `ERROR_CODE_MOVED` and `Error.moved` (where it went, when, and when this copy was deleted, read from `moved.json` each time). On the loopback port, a client whose Hello carries the token of the local pairing code the fence put away (`moved/remote/local-pairing`), this computer's own app, may then send `DeleteMovedCopy` with the word `delete`: the copy goes as with `--delete-moved`, and the connection closes.
2. **The fence.** `auth.json`, `vault/`, `account/` and `remote/` move into `<home>/moved/`, as they are. Then a backend of **any** version that starts here has no model sign-ins, no vault key, no account token and no tunnel key: it can't run an agent, open the secrets, check in, or answer devices as this Claktory. Nothing is deleted.
3. **The service** stays stopped, and is turned off so it doesn't start at login: `launchctl disable` on a Mac, `systemctl --user disable` on Linux.

Background tasks the source was running are stopped too.

The old copy stays like this until the person deletes it, once the moved
Claktory works. `factoryd export --restore` undoes a tombstone: the fence moves
back, `moved.json` goes, the service is turned on and starts. Use it only when
the moved Claktory never ran, or has been stopped for good.

## The handshake: moving over the network (phase 2)

Moving without carrying a file: the target runs `factoryd receive`, the
person pastes its code into the app of the running Claktory (the source),
and the two copy the Claktory between them while its agents keep working,
then switch over after a short freeze. It sends what a bundle holds (same
paths, same manifest), but file by file, in blocks, end to end encrypted by
the connection itself, so there's no passphrase. The shared code is
`crates/factoryd/src/move_wire.rs`. The source's side is
`move_source.rs`, and the target's is `move_import/receive.rs` plus import.

### The receive code

`factoryd receive` on the target makes a temporary endpoint with a key of
its own (`<home>/import/receive/key`), and prints its code as text and as a
QR code:

```
claktory-move://?e=<endpoint id>&s=<secret: 64 hex>&m=<machine>&v=<version>&o=<os>&h=<arch>[&r=<relay>]…[&a=<ip:port>]…
```

- `s` is 32 random bytes, kept beside the key, so running receive again shows the same code. A pairing that fails (digits turned down on either side, or the source gone before both confirmed) replaces it, and the new code is shown: whoever saw the old one can't ask again.
- The code has no expiry, and it's one-time: the first source key that proves the secret and has the digits confirmed on both sides claims it, and only that key can reconnect after that, to resume. While one source is pairing, another is turned away; a decline on either side frees the code again.
- The target talks to one source at a time: any other connection, while one is pairing or moving, gets `failed` ("Another computer is moving its Claktory here right now.") and no digits are shown for it. The source that claimed the code is the exception: a new connection from its key (it restarted, or its connection broke without a word) takes the place of its session at once, so it never waits for the old one to time out. An import or commit the old session started runs on, and the new session waits for it; a `commit` the source sent but didn't hear `done` for gets `done` again.
- `m`, `v`, `o` and `h` let the source's app say "Move to cloud-box (Linux x86_64)" and refuse an older target before it dials.
- Values are percent-encoded as in pairing codes (`remote::code_values`).

### The connection

- The source dials from a temporary endpoint of its own, never its tunnel's identity. Its key is `<home>/moving/key` (0600), kept across reconnects and the freeze, so it's the same source every time.
- ALPN `claktory-move/1`. The source opens one bidirectional stream, and both sides read and write it at the same time.
- **Frames:** a u32 big-endian length, then a JSON object with a `"type"`. A `block` frame is followed by `size` raw bytes. A header is at most 16 MiB.
- **Failure:** either side may send `failed {why}` at any time. `why` is a sentence for the person. The connection ends there; a source that hadn't committed carries on as it was.

### 1. Pairing

1. S→T `hello {secret, version, machine, os, arch}`.
2. T→S, one of:
   - `welcome {version, machine, os, arch, home, user_home, free_bytes, local_disk, fresh, resumed}`;
   - `failed {why}`, for any of: a wrong secret (after a one-second pause), a code another source key claimed, a target Claktory that's running or has work in it, a network disk, or a target older than the source.
3. **Digits.** Both sides work them out; they're never sent. SHA-256 of `"claktory move\n{target id}\n{source id}\n{secret hex}"`; its first 4 bytes, big-endian, modulo 1 000 000; shown as "123 456". A source that copied the code has another key, so it shows other digits.
4. Each side sends `confirmed {}` once its person has confirmed the digits: in the app on the source, and on the target's terminal. Either side may send `failed` instead.
5. A reconnect by the key that already claimed the code skips the digits: `welcome` has `resumed: true`. Any other key is then turned away with `failed`, even with the right secret.

### 2. Plan and checks

1. S→T `plan {manifest, map: [[from, to]…], tools: [name…]}`:
   - `manifest` is the manifest it would write;
   - `map` holds the folder destinations: by default the source's user home to the target's, and its home to the target's;
   - `tools` are the commands its agents' tasks used (`cargo`, `flutter`…).
2. T→S `checked {problems: [sentence…], map: [[from, to]…], missing: [tool…], free_bytes}`:
   - `problems` are import's own checks: the version, folder destinations free, disk against `requires.disk_bytes`, a local disk, this export_id not imported already. Empty means go.
   - `map` is the map in effect, longest prefix first.
   - `missing` are the tools the target doesn't have.

The source shows all of it to the person before copying starts.

### 3. Rounds

The target receives into `<home>/import/bundle/`, which has exactly the
bundle's layout (`manifest.json`, `home/…`, `folders/<n>/…`, `checkouts/…`),
so import goes on from there as it does from an unpacked bundle.

1. S→T `round {n, final}`. Then `entry {…}` for every path, and `round_end {n}`. Each round's listing is complete, so the final round's listing *is* the tree.
   - `entry`: `{path, kind: "file"|"dir"|"symlink", size, mode, mtime_ms, block, hashes: [hex…], target}`.
   - `block` is the file's block size: the smallest power of two from 128 KiB to 16 MiB that gives at most 2000 blocks.
   - `hashes` holds each block's SHA-256; it's empty for an empty file.
   - Paths and link targets follow the bundle's rules (relative, nothing that climbs out).
2. T→S `need {path, blocks: [index…]}` as it compares, while the listing still comes, then `need_end {n}`.
   - A block it already has is skipped: the same path, index and hash, or any block with that hash in its staging (a renamed file, say).
3. S→T `block {path, index, size}` and the bytes, for each needed block, then `blocks_end {n}`.
   - The target checks every block against the round's entry.
   - A mismatch in a round that isn't final means the file changed since the listing ("stale"): the block stays needed. In the final round it's `failed`.
4. T→S `round_done {n, bytes_have, bytes_total, stale}`: progress, for both sides.
5. After the final round, the target removes what's staged but isn't listed.
6. Staging never writes outside `import/bundle/`: a symlink entry's `target` must stay in the tree from where it is (`move_wire::safe_link`), and every write (an entry, a block, the end of a round) refuses a path with a symlink in a folder above it, and opens files without following one (`O_NOFOLLOW`). A block whose length or hash doesn't match its listing is refused before it's written; frames over 16 MiB before they're read.

**Resume:** the target keeps verified blocks: staged files, and an index of their block hashes by staged size and mtime (anything else is hashed again). A dropped connection, or receive run again, needs only what's missing. The source simply starts a new round.

### 4. Pre-copy, freeze, final round

On the source:

1. **Pre-copy, while agents work.** Rounds run in the normal backend, reading the Claktory live as a plain export does. Older conversation segments don't change, so later rounds send little. Pre-copy ends when a round sends less than 64 MiB or takes under 30 seconds, after at most 5 rounds.
2. **Freeze.** The source writes `<home>/moving/state.json` (phase `frozen`, the code, the export id), then exits the way an update does: runs are left open and every transcript is written. The supervisor starts it again, and at start, `moving/state.json` comes before everything else, `moved.json` included:
   - In **frozen mode** the backend opens no Factory, runs no agent and starts no identity endpoint. It answers the app with the move's progress.
   - Background tasks are stopped.
   - Agents' steps that were under way show "Interrupted", as after an update. On the target they carry on from there (the runs are open, so they resume).
3. **Final round.** From frozen mode, the source dials again (`resumed: true`) and sends the last round (`final: true`). SQLite databases are snapshotted again; git bundles and patches are made again.

### 5. Import, READY, commit

1. After the final round, the target runs import from Prepare to Verify. Meanwhile it streams `progress {step, text}`.
2. It then sends `ready {old_app_code}` or `failed {why}`. A failure before the home was switched over keeps what's staged, so the source can try again; one after it undoes the import here, and receive ends.
   - A source that reconnects after `ready` gets `ready` again.
   - Until it's committed, the target is never started: import's `state.json` stays, so its backend refuses to start.
3. On `ready`, the source commits:
   1. It fences itself and writes `moved.json` (as `--final` does: §"The source after `--final`"), with `to` and `to_os` naming the target.
   2. It sets `moving/state.json` to phase `committing`.
   3. It sends `commit {at_ms, export_id}`. The target records it and answers `done {}`, waits up to a minute for the source to hang up, and `receive` exits 0; then the target's service starts.
   - **`done` is the point of no return.** Before it, the source can still send `cancel {}`: the target throws everything away (as `import --abandon`), and the source unfreezes. After it, neither side goes back.
4. If the connection drops between `commit` and `done`, the source redials and sends `commit` again until it gets `done`. It does this from frozen mode, which a restart resumes while the phase is `committing`. Only then does it delete `moving/` and exit, so the supervisor finds `moved.json` and serves moved mode.
   - The target waits meanwhile, its endpoint up.
   - Last resort, on the target: `factoryd receive --committed`, once the person has seen the old machine say it moved.
5. **Unfreeze:** a failure or cancel before `commit` deletes `moving/` and exits. The normal backend starts and agents carry on, as after an update. The reason is kept in `<home>/move-failed.json` (`{at_ms, why}`), for the app to show.

### From the app (`move_run.rs`)

- `Workspace.move` (proto `Move`) shows the move from StartMove on; the
  phases follow the steps above. Pairing waits for ConfirmMove and for the
  target's `confirmed`, in either order; CancelMove before the commit calls
  it off (`failed` before pairing, `cancel` after) and clears the view.
- `moving/state.json` holds what must outlive a restart: the stage
  (`copying`, `frozen`, `committing`), the code, the export id, the next
  round, the epoch, the commit's time, and what the app was shown. A restart
  while copying carries on (the key is the same, so the target resumes it);
  pre-copy's rounds go on from the saved round's number. It dials for up to
  two minutes, as after a drop, since the target may still hold the
  connection the restart dropped.
- Progress: `bytes_have` and `bytes_total` move as blocks go within a round
  (at most once a second), counted from the blocks the target asked for;
  each round's end sets them to what the target says it has. While a
  dropped connection is dialed again, `step_text` says so, in pre-copy,
  the last round and import alike.
- The freeze: state `frozen`, the endpoint closed, then the backend exits as
  an update does (writes finished, the vault's key handed to the
  supervisor). Only a backend run by its supervisor can start a move
  (StartMove refuses otherwise).
- **Frozen mode** serves the loopback port only, and only to a client with
  this computer's own pairing token (`remote/local-pairing`, read before the
  fence): Welcome, then a Workspace with only `move` and `version`, again on
  every change. CancelMove is taken while `can_cancel`; everything else is
  refused. It starts no Factory, no tunnel and no agent, and stops leftover
  background tasks first.
- A failure or cancel in frozen mode deletes `moving/` and hands the vault's
  key back to the supervisor, so the Claktory starts again unlocked. A
  failure, in frozen mode or before it, is kept in `move-failed.json`
  (`{at_ms, why, target_machine}`) and shown as `MOVE_PHASE_FAILED`, after
  a restart too, until CancelMove dismisses it.
- After `done`, frozen mode shows `MOVE_PHASE_DONE` for two seconds, then
  exits; the next start finds `moved.json`.

### From a terminal: `factoryd move CODE`

`factoryd move CODE` drives the same handshake in the foreground, for a
Claktory with no app at hand. It asks for the digits on the terminal and
prints each round. Instead of frozen mode it freezes the way `--final`
does: it stops the service (and turns it off) and its background tasks,
sends the final round itself, and starts the service again if anything
fails before `commit`. A dropped connection is dialed again for two
minutes before a round gives up; after the fence it's dialed again until
`done`. Ctrl-C before the fence cancels the move.

### 6. After the move

- The moved Claktory keeps the same tunnel key, so devices find it on the new computer. The receive endpoint's id means nothing after the move.
- At its first start, the target asks the account service for a new token and key (`POST /v1/move`, signed with the old key), if it's signed in. The old copy's token then no longer works. Until that succeeds, it carries on with the old ones, retrying.
- The old copy stays fenced until the person deletes it (`factoryd export --delete-moved`, or from the app on that computer).
