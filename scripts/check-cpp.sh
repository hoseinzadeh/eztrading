#!/bin/bash
# Compile example strategies with the SML compiler and check that the generated C++ is valid
# against the runtime headers in package/. Usage: scripts/check-cpp.sh [file.sm ...]
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COMPILE="${EZT_COMPILER:-$ROOT/target/debug/compile}"
CXX="${CXX:-g++}"
files=("$@")
[ ${#files[@]} -eq 0 ] && files=("$ROOT/examples/threshold.sm" "$ROOT/examples/momentum.sm" "$ROOT/examples/pricelog.sm")
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
status=0
# the runtime itself (scheduler, signals, contents, ...)
for src in "$ROOT"/package/common/*.cpp; do
    flags=""; [ "$(basename "$src")" = scheduler.cpp ] && flags="-DSCHEDULER_INTERFACE"
    if "$CXX" -std=c++20 -fPIC -fsyntax-only $flags -I"$ROOT/package/inc" -I"$ROOT/package" "$src" 2> "$tmp/rt.err"; then
        echo "ok   runtime/$(basename "$src")"
    else
        echo "FAIL runtime/$(basename "$src")"; head -20 "$tmp/rt.err"; status=1
    fi
done
for f in "${files[@]}"; do
    name="$(basename "$f" .sm)"
    cp "$f" "$tmp/$name.sm"
    if ! (cd "$tmp" && "$COMPILE" -I "$ROOT/package/libs" "$name.sm" > "$name.log" 2>&1); then
        echo "FAIL $name: the SML compiler rejected it"; cat "$tmp/$name.log"; status=1; continue
    fi
    if "$CXX" -std=c++20 -fPIC -fsyntax-only -I"$ROOT/package/inc" -I"$tmp" "$tmp/$name.cpp" 2> "$tmp/$name.err"; then
        echo "ok   $name"
    else
        echo "FAIL $name: the generated C++ does not compile"; head -20 "$tmp/$name.err"; status=1
    fi
done
exit $status
