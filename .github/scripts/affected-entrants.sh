#!/usr/bin/env bash
# Map a push's diff to the benchmark work it invalidates.
#
#   .github/scripts/affected-entrants.sh <before-sha> <after-sha>
#
# Prints two lines on stdout:
#
#   selector=<bench selectors, space-separated, or '*' or empty>
#   trigger=<release|nightly>
#
# An empty selector means "this push moves no published number" and the launch
# workflow proposes nothing. The rules err toward over-proposing: the approval
# gate in front of every launch is the "is this re-run actually warranted"
# decision, and the plan job prints the exact arm list the money would buy —
# so a false positive costs a click, while a false negative silently leaves a
# stale number published.
set -euo pipefail

before=${1:?usage: affected-entrants.sh <before> <after>}
after=${2:?usage: affected-entrants.sh <before> <after>}

here=$(dirname "$0")

all=false
release=false
declare -A touched=()

# A spate version bump is invisible to path rules (it lives inside
# Cargo.lock), and it is the one change that gets `--trigger release`.
if ! diff -q \
  <("$here/spate-versions.sh" "$before") \
  <("$here/spate-versions.sh" "$after") >/dev/null; then
  touched[spate]=1
  release=true
fi

while IFS= read -r f; do
  case "$f" in
    # The comparability keys: harness code moves harness_version, the workload
    # moves dataset_version, the toolchain is provenance, and the instance
    # scripts define the box every arm runs on. Any of these invalidates
    # every published number at once.
    harness/*|workload/*|rust-toolchain.toml|.github/aws/*)
      all=true ;;
    # Only the cloud environment's own profile or ceilings force a
    # re-measurement on that environment. Other environments' files describe
    # hosts this pipeline does not run on.
    environments/c8g-*)
      all=true ;;
    entrants/*/*)
      e=${f#entrants/}
      touched["${e%%/*}"]=1 ;;
  esac
done < <(git diff --name-only "$before" "$after")

if $all; then
  echo "selector=*"
else
  selectors=""
  for e in "${!touched[@]}"; do
    # Only runnable entrants: a planned entrant's descriptor edits move
    # nothing measurable yet.
    status=$(sed -n 's/^status *= *"\(.*\)"/\1/p' "entrants/$e/entrant.toml" | head -1)
    if [ "$status" = active ]; then
      selectors="$selectors $e"
    fi
  done
  echo "selector=${selectors# }"
fi

if $release; then
  echo "trigger=release"
else
  echo "trigger=nightly"
fi
