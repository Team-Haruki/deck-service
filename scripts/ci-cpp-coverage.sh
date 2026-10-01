#!/usr/bin/env bash
# C++ bridge tests with gcov instrumentation, then a SonarQube generic coverage report.
# Ported from the old sonar.yml ("Run C++ bridge coverage tests" + "Generate SonarQube
# coverage report"). Needs gcovr on PATH and _cpp_src (scripts/prepare-cpp-engine.sh).
# Usage: scripts/ci-cpp-coverage.sh [output.xml]   (default coverage/cpp.xml)
set -euo pipefail
cd "$(dirname "$0")/.."
out="${1:-coverage/cpp.xml}"
export DECK_CPP_COVERAGE=1 CARGO_TARGET_DIR=target/coverage
mkdir -p target/coverage "$(dirname "$out")"
find target/coverage -name '*.gcda' -delete
cargo test --locked --test cpp_bridge
bridge_library="$(find target/coverage/debug/build -path '*/out/native-cpp/lib/libdeck_recommend.a' -print -quit)"
[ -n "$bridge_library" ] || { echo "libdeck_recommend.a not found under target/coverage" >&2; exit 1; }
test_dir="target/coverage/cpp-tests"
mkdir -p "$test_dir"
c++ -std=c++20 -O0 -g --coverage -fno-sanitize=all \
  -I _cpp_src/src -I _cpp_src/3rdparty/yyjson/src -I cpp_bridge \
  -c tests/deck_recommend_c_test.cpp -o "$test_dir/deck_recommend_c_test.o"
c++ --coverage "$test_dir/deck_recommend_c_test.o" "$bridge_library" -pthread \
  -o "$test_dir/deck_recommend_c_test"
"$test_dir/deck_recommend_c_test"
gcovr --root "$PWD" --filter "$PWD/cpp_bridge/" \
  --sonarqube-metric line --sonarqube "$out" --print-summary target/coverage
