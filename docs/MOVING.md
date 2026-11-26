# Moving your Claktory to another computer

How to move a Claktory — agents, conversations, secrets, sign-ins and project
folders — to a new Mac or a Linux server, from Terminal. The bundle's format
and the move protocol are `docs/MOVE.md`; this is the doing. These commands
exist in releases after v0.1.13.

There are two ways:

- **Over the network** (the usual way): the new computer shows a code, you
  paste it into your running Claktory, and it moves itself — agents keep
  working while it copies. Start with "Moving over the network" below.
- **By bundle file**: the old computer packs everything into one encrypted
  file you carry over. For when the two computers can't reach each other, or
  the old one is being retired on the spot. Start with "Moving by bundle".

## Moving over the network

On the **new** computer, one line installs Claktory and waits for the move:

```sh
curl -fsSL https://github.com/claktory/claktory/releases/latest/download/install.sh | bash -s -- --receive
```

It installs the newest release held (not started) and shows a move code,
also as a QR code. Leave it running.

On the **old** computer, open Claktory and choose **Move to another
computer…** (Settings → General), then paste the code — or scan the QR with
Claktory on your iPhone. Both sides show the same digits; confirm they
match. Your agents keep working while everything copies; at the end the old
Claktory freezes briefly, the move switches over, and Claktory starts on the
new computer. Your devices find it there by themselves.

If the terminal side stops (Ctrl-C, a dropped connection), nothing is lost:
run the same install command again and it carries on — a move already under
way resumes where it stopped; a code nobody had used yet is replaced by a
fresh one. From a terminal on the old computer, `factoryd move CODE` does
what the app's Move steps do.

### Letting Rivet set up the new computer

When the new computer is a server, Rivet can do its side over SSH, so you
never open a terminal there:

1. Rivet asks you for an SSH key for the server and has Claktory's vault
   generate it: nobody sees the private half, and you add the public half
   Rivet shows you to the server (most hosts ask for it when you create the
   server; otherwise to root's `~/.ssh/authorized_keys`).
2. Its first connection accepts the server's host key and tells you the key's
   fingerprint, so you can check it against what your host shows. A key that
   changes later is refused.
3. It runs the installer on the server in the background, kept open for your
   answer:

   ```sh
   ssh root@SERVER 'bash -c "$(curl -fsSL https://claktory.com/install.sh)" -- --receive --json'
   ```

   (`bash -c`, not `| bash`: the installer reads your yes or no about the
   digits on its input, which piped into bash would be the script itself.)
   It answers in JSON lines: the move code, how it's going, and the digits to
   compare; the last line says Claktory runs on the server, or why the move
   stopped.
4. The move starts from this Claktory with that code, as in Move to another
   computer…. Both sides show the same six digits; you check them, and Rivet
   passes your yes or no to the server, never its own.
5. Your agents, Rivet included, carry on on the server once it switches over.
   The installer refuses a server that already runs a Claktory, so it never
   stops one.

## Moving by bundle

### Before you start

- You need a few minutes when Claktory can stop: exporting stops it on the old
  computer for good (everything is kept).
- Have somewhere to write the passphrase down. It's shown once and kept
  nowhere.

### On the old computer

Open Terminal (not an agent: the command refuses to run from inside Claktory)
and run:

```sh
factoryd export --final
```

It stops Claktory, packs everything into one encrypted file,
`claktory-<machine>-<date>.claktory` in the current folder (`--out FILE`
chooses another place, `--skip FOLDER` leaves a project folder out), and
prints the **passphrase** once. Write it down: the bundle can't be opened
without it.

The old Claktory is now stopped and fenced: it keeps everything but runs
nothing and answers no devices. If you change your mind before importing,
`factoryd export --restore` brings it back as it was.

### Copy the bundle

Copy the `.claktory` file to the new computer any way you like: `scp`,
AirDrop, a USB disk. It's encrypted; without the passphrase it doesn't open.

```sh
scp claktory-*.claktory you@new-computer:
```

### On the new computer

One command installs Claktory and brings the bundle in:

```sh
curl -fsSL https://github.com/claktory/claktory/releases/latest/download/install.sh | bash -s -- --import ~/claktory-<machine>-<date>.claktory
```

It installs the newest release held (not started), asks for the passphrase,
imports everything, and then starts Claktory. If your project folders live at
different paths on the new computer, say where with `--map`, repeatable:

```sh
… | bash -s -- --import ~/bundle.claktory --map /Users/you/work=/home/you/work
```

By default the old computer's home folder maps to the new one's, which covers
most moves, Mac to Linux included. `--import` needs a terminal for the
passphrase, so it can't run with `--json`; an agent moving a Claktory runs
`factoryd import` itself after installing with `--hold`.

If Claktory is already installed on the new computer but has never been set
up, run the import directly instead: `factoryd import ~/bundle.claktory`, with
Claktory stopped.

The import ends with a pairing code: that one is for the **old** computer's
app, so it can connect to the moved Claktory. Your other devices — iPhone,
other Macs — find the moved Claktory by themselves, without being added again.

## Check it, then let the old copy go

(Both ways of moving end here.)

On the new computer, open the app (or run `factoryd status`): your projects
and agents are there, and each agent got a note saying it moved, with anything
that didn't survive named (background tasks don't move; they're over, and the
agents are told).

Once the moved Claktory works, delete the old copy on the old computer:

```sh
factoryd export --delete-moved
```

Until then the old copy stays fenced as a fallback. `factoryd export
--restore` revives it — but only when the moved Claktory never ran, or has
been stopped for good. Never run both: they'd be two Claktories with the same
identity, and your devices and account can only follow one.

## When something goes wrong

- **Wrong passphrase, or a damaged bundle:** one refusal, nothing changed.
  Check the passphrase, or copy the bundle over again.
- **The import was interrupted** (the connection dropped, the machine
  restarted): run the same install command again and it resumes where it
  stopped. To undo the half-import instead: `factoryd import --abandon`.
  Claktory refuses to start while an import is unfinished, so nothing runs on
  half a move.
- **Import refuses the bundle** (too old a release on the new computer, not
  enough disk): it says so in one sentence before touching anything. Install
  the newest release, or free the space, and run it again.
- **The move is off:** `factoryd export --restore` on the old computer brings
  the old Claktory back, as long as the moved one never ran.
