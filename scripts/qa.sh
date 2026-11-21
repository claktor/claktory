#!/usr/bin/env bash
# Runs claktory-qa (crates/claktory-qa/README.md) as built from a commit:
# what's committed, never the working tree. The first run for a commit
# exports it with `git archive` and builds factoryd, claktory-account and
# claktory-qa from it into the QA root; later runs reuse them.
#
#   scripts/qa.sh up                    a sandbox on main's HEAD
#   scripts/qa.sh --rev factory/me up   on another branch or commit
#   scripts/qa.sh send qa1 CreateProject '{"name":"Site"}'
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
# Everything lives under CLAKTORY_QA_ROOT (default: claktory-qa in the
# temporary folder): never among the real Claktories' own files in
# ~/.factory or ~/.factory-dev (agents' folders there are fine).
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="${CLAKTORY_QA_ROOT:-${TMPDIR:-/tmp}/claktory-qa}"
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
  [[ $# -gt 0 ]] || exec "$ENGINE" exec -e CLAKTORY_QA_ROOT=/qa -e CLAKTORY_QA_COMMIT="$COMMIT" "$NAME" /opt/claktory-qa/claktory-qa
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
  exec "$ENGINE" exec -e CLAKTORY_QA_ROOT=/qa -e CLAKTORY_QA_COMMIT="$COMMIT" "$NAME" /opt/claktory-qa/claktory-qa "${args[@]}"
fi

BIN="$ROOT/bin/$COMMIT"
if [[ ! -x "$BIN/claktory-qa" ]]; then
  SRC="$ROOT/src/$COMMIT"
  rm -rf "$SRC"
  mkdir -p "$SRC"
  git -C "$REPO" archive "$COMMIT" | tar -x -C "$SRC"
  echo "claktory-qa: building $COMMIT (once per commit; the first takes a few minutes)" >&2
  # No update channel stamped in: a sandbox never looks for releases.
  (cd "$SRC" && env -u FACTORY_UPDATE_URL CARGO_TARGET_DIR="$ROOT/target" \
    cargo build --quiet -p factoryd -p claktory-account -p claktory-qa) >&2
  rm -rf "$BIN.tmp"
  mkdir -p "$BIN.tmp"
  cp "$ROOT/target/debug/factoryd" "$ROOT/target/debug/claktory-account" "$ROOT/target/debug/claktory-qa" "$BIN.tmp/"
  mv "$BIN.tmp" "$BIN"
  rm -rf "$SRC"
fi

export CLAKTORY_QA_ROOT="$ROOT" CLAKTORY_QA_COMMIT="$COMMIT"
exec "$BIN/claktory-qa" "$@"
