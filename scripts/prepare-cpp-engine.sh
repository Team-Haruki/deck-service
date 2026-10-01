#!/usr/bin/env bash
# Clone the pinned sekai-deck-recommend-cpp engine (with submodules) into _cpp_src.
# Ported from the "Prepare C++ engine source" step of the old ci.yml / sonar.yml / release.yml.
# cpp-engine.ref is the only place the engine commit is written (the Dockerfile reads it too).
set -euo pipefail
cd "$(dirname "$0")/.."
repo="${DECK_CPP_REPO:-https://github.com/Team-Haruki/sekai-deck-recommend-cpp.git}"
branch="${DECK_CPP_BRANCH:-master}"
ref="$(tr -d '[:space:]' < cpp-engine.ref)"
if ! [[ "$ref" =~ ^[0-9a-f]{40}$ ]]; then
  echo "::error file=cpp-engine.ref::expected a full 40-char commit SHA, got '$ref'"
  exit 1
fi
if [[ -d _cpp_src/.git && "$(git -C _cpp_src rev-parse HEAD)" == "$ref" ]]; then
  echo "_cpp_src already at $ref"; exit 0
fi
rm -rf _cpp_src
git clone --branch "$branch" --single-branch "$repo" _cpp_src
git -C _cpp_src checkout "$ref"
git -C _cpp_src submodule update --init --recursive
