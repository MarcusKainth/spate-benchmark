#!/usr/bin/env bash
# Print "<crate> <version>" for the SUT crates recorded in a ref's Cargo.lock.
#
#   .github/scripts/spate-versions.sh <git-ref>
#
# The lockfile is the provenance of which spate the arm measures, so "did a
# push change the spate version" is answered here and nowhere else — a path
# filter cannot see WHICH dependency moved inside Cargo.lock.
set -euo pipefail

ref=${1:?usage: spate-versions.sh <git-ref>}

git show "$ref:Cargo.lock" | awk '
  /^name = /    { name = $3; gsub(/"/, "", name) }
  /^version = / {
    if (name == "spate-core" || name == "spate-avro" ||
        name == "spate-kafka" || name == "spate-clickhouse") {
      version = $3; gsub(/"/, "", version)
      print name, version
    }
  }
' | sort
