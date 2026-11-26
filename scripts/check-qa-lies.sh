#!/usr/bin/env bash
# Checks that no release build can carry a QA-only lying feature (Bruno
# 40066): each crate's feature below must fail to compile with --release,
# with its guard's message, so a deploy never ships a worker or backend that
# lies on purpose. Run it after changing a feature or its guard.
#
#   scripts/check-qa-lies.sh
set -euo pipefail
cd "$(dirname "$0")/.."

# crate:feature, each guarded by a compile_error! in non-debug builds that
# says "for QA builds only".
checks=(
  claktory-user-apps:qa-lies
  factoryd:qa-lies
  factoryd:qa-idp-fill
)

failed=0
for check in "${checks[@]}"; do
  crate="${check%%:*}"
  feature="${check#*:}"
  if output=$(cargo check --release -p "$crate" --features "$feature" 2>&1); then
    echo "FAIL: a release build of $crate with $feature compiles" >&2
    failed=1
  elif grep -q "for QA builds only" <<<"$output"; then
    echo "ok: a release build of $crate with $feature is refused"
  else
    echo "FAIL: a release build of $crate with $feature fails, but not at its guard:" >&2
    grep -E "^error" <<<"$output" | sed -n 1,10p >&2
    failed=1
  fi
done
exit "$failed"
