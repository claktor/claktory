#!/usr/bin/env bash
# Runs claktory-qa (crates/claktory-qa/README.md) as built from a commit:
# what's committed, never the working tree. The first run for a commit
# exports it with `git archive` and builds factoryd, claktory-account and
# claktory-qa from it into the QA root; later runs reuse them.
#
#   scripts/qa.sh up                    a sandbox on main's HEAD
#   scripts/qa.sh --rev factory/me up   on another branch or commit
#   scripts/qa.sh send qa1 CreateProject '{"name":"Site"}'
#   scripts/qa.sh up --speech ~/.factory-dev/models/speech/parakeet-tdt-0.6b-v3-Q8_0.gguf
#                                       speech recognition ready (claktory-speech
#                                       built once per commit, the model copied in)
#   scripts/qa.sh up --claude           Claude signed in, fake, on qa-model (factoryd
#                                       built with the QA provider override, debug only);
#                                       scripts/qa.sh limit qa1 --for 120 puts it at its
#                                       usage limit
#   scripts/qa.sh up --version v0.1.17  a sandbox whose factoryd says it's v0.1.17,
#                                       as a release's does (built once per commit
#                                       and version), so apps of other versions
#                                       meet the checks they meet on a real one;
#                                       a sandbox otherwise says "dev"
#
# --container runs the same commands inside a Linux container instead
# (podman): claktory-qa, its sandboxes and their agents' tools all run in
# there, and the container has no network at all (--network none), so
# nothing real is reachable from inside -- the tier for QA runs whose
# scripted agents execute real tools. The first run for a commit builds its
# static Linux binaries (scripts/build-linux.sh) and an image from them.
# The container (named claktory-qa) stays up between commands; switching
# commits replaces it, and `podman rm -f claktory-qa` removes it.
#
#   scripts/qa.sh --container up --no-account --model
#   scripts/qa.sh --container ask qa1 Lea 'call bash {"command": "ls"}'
#   scripts/qa.sh --container pull /qa/sandboxes/qa1/factoryd.log
#   scripts/qa.sh --container down --all
#
# Everything lives under CLAKTORY_QA_ROOT (default: ~/Library/Caches/
# claktory-qa on macOS, $XDG_CACHE_HOME/claktory-qa or ~/.cache/claktory-qa
# elsewhere): never among the real Claktories' own files in ~/.factory or
# ~/.factory-dev (agents' folders there are fine). Not the temporary folder:
# macOS deletes files there it thinks are unused, a generated file in the
# middle of the build cache included, and every build then fails.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
if [[ "$(uname -s)" == Darwin ]]; then
  ROOT="${CLAKTORY_QA_ROOT:-$HOME/Library/Caches/claktory-qa}"
else
  ROOT="${CLAKTORY_QA_ROOT:-${XDG_CACHE_HOME:-$HOME/.cache}/claktory-qa}"
fi
REV=HEAD
CONTAINER=""
while [[ "${1:-}" == "--rev" || "${1:-}" == "--container" ]]; do
  case "$1" in
    --container)
      CONTAINER=1
      shift
      ;;
    --rev)
      REV="${2:?--rev needs a branch or commit}"
      shift 2
      ;;
  esac
done

# `up --version vX.Y.Z`: the version stamped into this sandbox's binaries
# (only that number: no update channel, nothing else of a release). Taken
# out of the arguments here; claktory-qa never sees it.
VERSION=""
if [[ "${1:-}" == "up" ]]; then
  rest=()
  while [[ $# -gt 0 ]]; do
    if [[ "$1" == "--version" ]]; then
      VERSION="${2:-}"
      if [[ ! "$VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        echo '{"ok":false,"error":"usage","message":"--version takes a release version, such as v0.1.17."}'
        exit 2
      fi
      shift 2
    else
      rest+=("$1")
      shift
    fi
  done
  set -- "${rest[@]}"
fi

mkdir -p "$ROOT"
ROOT="$(cd "$ROOT" && pwd -P)"
for real in "$HOME/.factory" "$HOME/.factory-dev"; do
  real="$(cd "$real" 2>/dev/null && pwd -P || echo "$real")"
  # Agents' folders and checkouts are theirs; the rest is the real Claktory's.
  case "$ROOT/" in
    "$real/agents/"* | "$real/trees/"* | "$real/workspace/"*) ;;
    "$real/"*)
      echo "{\"ok\":false,\"error\":\"refused\",\"message\":\"$ROOT is among the files of the real Claktory in $real.\"}"
      exit 3
      ;;
  esac
done

COMMIT="$(git -C "$REPO" rev-parse --short=10 "$REV^{commit}")"

if [[ -n "$CONTAINER" && -n "$VERSION" ]]; then
  echo '{"ok":false,"error":"usage","message":"--version runs only outside the container for now."}'
  exit 2
fi

if [[ -n "$CONTAINER" ]]; then
  ENGINE="${FACTORY_CONTAINER:-podman}"
  if ! command -v "$ENGINE" >/dev/null; then
    echo "the container tier needs podman (brew install podman; podman machine init; podman machine start)" >&2
    exit 1
  fi
  IMAGE="localhost/claktory-qa:$COMMIT"
  NAME=claktory-qa
  if ! "$ENGINE" image inspect "$IMAGE" >/dev/null 2>&1; then
    # The commit's own source (git archive, never the working tree), built
    # static for Linux by this checkout's build-linux.sh.
    SRC="$ROOT/src/$COMMIT-linux"
    LIN="$ROOT/linux/$COMMIT"
    rm -rf "$SRC"
    mkdir -p "$SRC" "$LIN"
    git -C "$REPO" archive "$COMMIT" | tar -x -C "$SRC"
    echo "claktory-qa: building the $COMMIT image (once per commit; the first takes a while)" >&2
    case "$(uname -m)" in
    arm64 | aarch64) LARCH=aarch64 ;;
    *) LARCH=x86_64 ;;
    esac
    for pkg in factoryd claktory-account claktory-qa; do
      # No update channel stamped in: a sandbox never looks for releases.
      env -u FACTORY_UPDATE_URL -u FACTORY_VERSION FACTORY_LINUX_SRC="$SRC" \
        "$REPO/scripts/build-linux.sh" "$LARCH" "$LIN/$pkg" "$pkg" >&2
    done
    rm -rf "$SRC"
    cat >"$LIN/Containerfile" <<'EOF'
# The contained QA tier, built by scripts/qa.sh --container: claktory-qa,
# factoryd and the account service, static, plus what agents' tools run on.
FROM docker.io/library/alpine:3.21
# procps-ng: the sandbox checks its processes with `ps -o command= -p`,
# which busybox's ps can't do.
RUN apk add --no-cache bash coreutils git procps-ng && mkdir -p /qa
COPY factoryd claktory-account claktory-qa /opt/claktory-qa/
EOF
    "$ENGINE" build -q -t "$IMAGE" "$LIN" >&2
  fi
  RAN="$("$ENGINE" container inspect "$NAME" --format '{{.Config.Image}}' 2>/dev/null || true)"
  if [[ -n "$RAN" && "$RAN" != "$IMAGE" ]]; then
    echo "claktory-qa: the container ran $RAN; replacing it for $COMMIT (its sandboxes are gone)" >&2
    "$ENGINE" rm -f "$NAME" >/dev/null
    RAN=""
  fi
  if [[ -z "$RAN" ]]; then
    # --network none: nothing outside the container is reachable from inside,
    # this Mac included. The driver comes in through exec instead of a port.
    "$ENGINE" run -d --name "$NAME" --network none "$IMAGE" sleep infinity >/dev/null
  elif [[ "$("$ENGINE" container inspect "$NAME" --format '{{.State.Running}}')" != true ]]; then
    "$ENGINE" start "$NAME" >/dev/null
  fi
  for arg in "$@"; do
    if [[ "$arg" == "--speech" ]]; then
      echo '{"ok":false,"error":"usage","message":"--speech runs only outside the container: its claktory-speech and model are this Mac'\''s."}'
      exit 2
    fi
  done
  [[ $# -gt 0 ]] || exec "$ENGINE" exec -e CLAKTORY_QA_ROOT=/qa -e CLAKTORY_QA_COMMIT="$COMMIT" -e CLAKTORY_QA_REPO="$REPO" -e CLAKTORY_QA_OWNER="${CLAKTORY_QA_OWNER:-}" "$NAME" /opt/claktory-qa/claktory-qa
  # The paths a command's JSON names (log, mail, model_script) are container
  # paths: pull copies one out to this Mac.
  if [[ "$1" == "pull" ]]; then
    SRC="${2:-}"
    if [[ -z "$SRC" ]]; then
      echo '{"ok":false,"error":"usage","message":"pull PATH [DEST]: copy a container path (what a command'\''s JSON names) out to this Mac."}'
      exit 2
    fi
    DEST="${3:-$(basename "$SRC")}"
    # Absolute, through this Mac's symlinks (/tmp): podman cp trips on them.
    [[ "$DEST" == /* ]] || DEST="$(pwd -P)/$DEST"
    if ! "$ENGINE" cp "$NAME:$SRC" "$DEST" >/dev/null; then
      echo "{\"ok\":false,\"error\":\"failed\",\"message\":\"Can't copy $SRC out of the container.\"}"
      exit 1
    fi
    echo "{\"ok\":true,\"pulled\":\"$DEST\"}"
    exit 0
  fi
  # A qa-model script the person names lives on this Mac: copy it inside
  # first (up ... --script FILE, and script NAME FILE).
  args=("$@")
  for ((i = 1; i < ${#args[@]}; i++)); do
    want=""
    [[ "${args[i - 1]}" == "--script" ]] && want=1
    [[ "${args[0]}" == "script" && $i -eq 2 ]] && want=1
    if [[ -n "$want" && -f "${args[i]}" ]]; then
      inside="/tmp/qa-script-$$-$i.json"
      "$ENGINE" cp "${args[i]}" "$NAME:$inside"
      args[i]="$inside"
    fi
  done
  exec "$ENGINE" exec -e CLAKTORY_QA_ROOT=/qa -e CLAKTORY_QA_COMMIT="$COMMIT" -e CLAKTORY_QA_REPO="$REPO" -e CLAKTORY_QA_OWNER="${CLAKTORY_QA_OWNER:-}" "$NAME" /opt/claktory-qa/claktory-qa "${args[@]}"
fi

# A commit's binaries are reused only when its build vouched for them:
# $BIN/commit names the commit, written once every crate of its source was
# compiled in that build (below). Folders without it, such as those built
# before this check, are built again.
# With --version, a folder of their own: the same commit, that number.
STAMP="$COMMIT${VERSION:+-$VERSION}"
BIN="$ROOT/bin/$STAMP"
# The cache prunes itself as it grows (the shared disk once filled up):
# before each new commit's build, it drops other commits' binaries older
# than CLAKTORY_QA_KEEP_HOURS (4) that no running process uses, then the
# oldest of the rest beyond CLAKTORY_QA_KEEP_BUILDS (6), and when the
# shared target is over CLAKTORY_QA_TARGET_GB (15), its incremental
# state, then all of it (the next build is a cold one). Called holding the
# build lock.
prune_cache() {
  local running dir commit kb
  running="$(ps -axo command= 2>/dev/null | grep -oE "$ROOT/bin/[0-9a-f]+(-v[0-9.]+)?/" | sort -u || true)"
  while IFS= read -r dir; do
    commit="$(basename "$dir")"
    [[ "$commit" != "$STAMP" ]] || continue
    grep -qF "$ROOT/bin/$commit/" <<<"$running" && continue
    rm -rf "$dir"
  done < <(find "$ROOT/bin" -mindepth 1 -maxdepth 1 -type d -mmin +$((${CLAKTORY_QA_KEEP_HOURS:-4} * 60)) 2>/dev/null)
  # At most CLAKTORY_QA_KEEP_BUILDS, newest first; this build's slot counts.
  local kept=1
  while IFS= read -r dir; do
    commit="$(basename "$dir")"
    [[ "$commit" != "$STAMP" ]] || continue
    grep -qF "$ROOT/bin/$commit/" <<<"$running" && continue
    if ((kept < ${CLAKTORY_QA_KEEP_BUILDS:-6})); then kept=$((kept + 1)); continue; fi
    rm -rf "$dir"
  done < <(ls -1dt "$ROOT"/bin/*/ 2>/dev/null | sed 's:/$::')
  [[ -d "$ROOT/target" ]] || return 0
  kb="$(du -sk "$ROOT/target" 2>/dev/null | cut -f1)"
  if ((kb > ${CLAKTORY_QA_TARGET_GB:-15} * 1024 * 1024)); then
    rm -rf "$ROOT/target/debug/incremental"
    kb="$(du -sk "$ROOT/target" 2>/dev/null | cut -f1)"
    if ((kb > ${CLAKTORY_QA_TARGET_GB:-15} * 1024 * 1024)); then
      echo "claktory-qa: the shared build cache is over ${CLAKTORY_QA_TARGET_GB:-15} GB; starting it afresh" >&2
      rm -rf "$ROOT/target"
    fi
  fi
}
built() { [[ -x "$BIN/claktory-qa" && "$(cat "$BIN/commit" 2>/dev/null)" == "$STAMP" ]]; }
if ! built; then
  # One build at a time in the shared target, from cargo to the copy out
  # of it: another commit's build in between would leave its binaries
  # where this one copies from. A lock whose holder is gone is taken over.
  LOCK="$ROOT/build.lock"
  waited=""
  until mkdir "$LOCK" 2>/dev/null; do
    holder="$(cat "$LOCK/pid" 2>/dev/null || true)"
    if [[ -n "$holder" ]] && ! kill -0 "$holder" 2>/dev/null; then
      rm -rf "$LOCK"
      continue
    fi
    [[ -n "$waited" ]] || echo "claktory-qa: waiting for another build ($LOCK)" >&2
    waited=1
    sleep 1
  done
  echo $$ >"$LOCK/pid"
  trap 'rm -rf "$LOCK"' EXIT
  if ! built; then
    prune_cache
    SRC="$ROOT/src/$STAMP"
    rm -rf "$SRC" "$BIN"
    mkdir -p "$SRC"
    # -m: the files are new now. git archive stamps them with the commit's
    # time, and cargo, judging by times, would find an older commit's
    # source no newer than the shared target's last build: it would then
    # rebuild nothing, or only some crates, and give another commit's
    # binaries, or a mix of two.
    git -C "$REPO" archive "$COMMIT" | tar -xm -C "$SRC"
    echo "claktory-qa: building $COMMIT (once per commit; the first takes a few minutes)" >&2
    MESSAGES="$ROOT/src/$STAMP.json"
    # No update channel stamped in: a sandbox never looks for releases. No
    # version either, but the one --version asks for. With the QA provider
    # override (qa-provider, debug builds only), where the commit has it:
    # `up --claude` signs Claude in, fake, on qa-model.
    FEATURES=()
    if grep -q '^qa-provider' "$SRC/crates/factoryd/Cargo.toml"; then FEATURES=(--features factoryd/qa-provider); fi
    (cd "$SRC" && env -u FACTORY_UPDATE_URL -u FACTORY_VERSION ${VERSION:+FACTORY_VERSION="$VERSION"} CARGO_TARGET_DIR="$ROOT/target" \
      cargo build --quiet --message-format=json-render-diagnostics \
      -p factoryd -p claktory-account -p claktory-qa ${FEATURES[@]+"${FEATURES[@]}"}) >"$MESSAGES"
    # The proof: every crate of this source compiled in this build, none
    # found fresh from another's, and the three binaries among them.
    ours="$(grep '"reason":"compiler-artifact"' "$MESSAGES" | grep -F "\"package_id\":\"path+file://$SRC/" || true)"
    stale="$(grep -F '"fresh":true' <<<"$ours" | sed -E 's/.*"package_id":"path\+file:\/\/[^"]*\/([^"#\/]+)#.*/\1/' | sort -u | tr '\n' ' ' || true)"
    for bin in factoryd claktory-account claktory-qa; do
      grep -F '"fresh":false' <<<"$ours" | grep -qF "\"executable\":\"$ROOT/target/debug/$bin\"" ||
        stale="$stale$bin(not built) "
    done
    if [[ -n "$stale" ]]; then
      echo "{\"ok\":false,\"error\":\"stale_build\",\"message\":\"The build of $COMMIT reused what another commit built: ${stale% }. Nothing was kept; its cargo messages are in $MESSAGES.\"}"
      rm -rf "$SRC"
      exit 1
    fi
    rm -rf "$BIN.tmp"
    mkdir -p "$BIN.tmp"
    cp "$ROOT/target/debug/factoryd" "$ROOT/target/debug/claktory-account" "$ROOT/target/debug/claktory-qa" "$BIN.tmp/"
    echo "$STAMP" >"$BIN.tmp/commit"
    mv "$BIN.tmp" "$BIN"
    rm -rf "$SRC" "$MESSAGES"
  fi
  rm -rf "$LOCK"
  trap - EXIT
fi

# `up --speech MODEL` also needs the commit's claktory-speech, which builds
# transcribe.cpp (ggml) from source: minutes, so only when a sandbox asks
# for it, once per commit, vouched for the same way.
speech=""
if [[ "${1:-}" == "up" ]]; then
  for arg in "$@"; do
    if [[ "$arg" == "--speech" ]]; then speech=1; fi
  done
fi
if [[ -n "$speech" && ! -x "$BIN/claktory-speech" ]]; then
  LOCK="$ROOT/build.lock"
  waited=""
  until mkdir "$LOCK" 2>/dev/null; do
    holder="$(cat "$LOCK/pid" 2>/dev/null || true)"
    if [[ -n "$holder" ]] && ! kill -0 "$holder" 2>/dev/null; then
      rm -rf "$LOCK"
      continue
    fi
    [[ -n "$waited" ]] || echo "claktory-qa: waiting for another build ($LOCK)" >&2
    waited=1
    sleep 1
  done
  echo $$ >"$LOCK/pid"
  trap 'rm -rf "$LOCK"' EXIT
  if [[ ! -x "$BIN/claktory-speech" ]]; then
    SRC="$ROOT/src/$COMMIT-speech"
    rm -rf "$SRC"
    mkdir -p "$SRC"
    git -C "$REPO" archive "$COMMIT" | tar -xm -C "$SRC"
    echo "claktory-qa: building claktory-speech at $COMMIT (once per commit; the first builds ggml and takes several minutes)" >&2
    MESSAGES="$ROOT/src/$COMMIT-speech.json"
    (cd "$SRC" && CARGO_TARGET_DIR="$ROOT/target" \
      cargo build --quiet --message-format=json-render-diagnostics -p claktory-speech) >"$MESSAGES"
    ours="$(grep '"reason":"compiler-artifact"' "$MESSAGES" | grep -F "\"package_id\":\"path+file://$SRC/" || true)"
    stale="$(grep -F '"fresh":true' <<<"$ours" | sed -E 's/.*"package_id":"path\+file:\/\/[^"]*\/([^"#\/]+)#.*/\1/' | sort -u | tr '\n' ' ' || true)"
    grep -F '"fresh":false' <<<"$ours" | grep -qF "\"executable\":\"$ROOT/target/debug/claktory-speech\"" ||
      stale="${stale}claktory-speech(not built) "
    if [[ -n "$stale" ]]; then
      echo "{\"ok\":false,\"error\":\"stale_build\",\"message\":\"The build of claktory-speech at $COMMIT reused what another commit built: ${stale% }. Nothing was kept; its cargo messages are in $MESSAGES.\"}"
      rm -rf "$SRC"
      exit 1
    fi
    cp "$ROOT/target/debug/claktory-speech" "$BIN/claktory-speech.tmp"
    mv "$BIN/claktory-speech.tmp" "$BIN/claktory-speech"
    rm -rf "$SRC" "$MESSAGES"
  fi
  rm -rf "$LOCK"
  trap - EXIT
fi

export CLAKTORY_QA_ROOT="$ROOT" CLAKTORY_QA_COMMIT="$COMMIT" CLAKTORY_QA_REPO="$REPO"
exec "$BIN/claktory-qa" "$@"
