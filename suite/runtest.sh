#!/bin/sh
# Run Redis's own test files against our server.
#
# The suite normally starts the server it tests, from a Redis config file with
# Redis options in it. We are not Redis and do not read that file, so this uses
# the suite's external mode: we start yodb, and `runtest --host --port` points
# the tests at it.
#
# Every failure here is real. A test that fails because we have not implemented
# the command yet is still a compatibility failure, it is just one with a date
# on it. Nothing is patched out to make the number look better. The list of
# files that currently pass lives in suite/passing.txt, and moving a file out of
# that list is a decision somebody makes on purpose in a diff.
#
# Usage: suite/runtest.sh [--all] [file ...]
#   with no arguments it runs the files in suite/passing.txt

set -eu

HERE="$(cd "$(dirname "$0")/.." && pwd)"
YODB="${YODB:-$HERE/../yo/target/release/yodb}"
PORT="${PORT:-7511}"
REDIS="$HERE/redis"

[ -d "$REDIS/tests" ] || { echo "run suite/fetch.sh first" >&2; exit 2; }
[ -x "$YODB" ] || { echo "no yodb at $YODB. Set YODB." >&2; exit 2; }

ALL=no
files=""
for arg in "$@"; do
  case "$arg" in
    --all) ALL=yes ;;
    -*) echo "runtest: no such option: $arg" >&2; exit 2 ;;
    *) files="$files $arg" ;;
  esac
done

if [ -z "$files" ]; then
  if [ "$ALL" = yes ]; then
    files="$(cd "$REDIS/tests" && find unit -name '*.tcl' | sed 's|\.tcl$||' | sort | tr '\n' ' ')"
  else
    files="$(grep -v '^#' "$HERE/suite/passing.txt" | grep -v '^$' | tr '\n' ' ')"
  fi
fi

"$YODB" serve --port "$PORT" >/tmp/yo-compat-server.log 2>&1 &
server=$!
trap 'kill $server 2>/dev/null || true' EXIT

# Wait for it rather than sleeping a guess.
i=0
while [ $i -lt 100 ]; do
  if printf 'PING\r\n' | timeout 1 nc -q1 127.0.0.1 "$PORT" 2>/dev/null | grep -q PONG; then
    break
  fi
  i=$((i + 1))
  sleep 0.05
done
[ $i -lt 100 ] || { echo "yodb never answered on $PORT" >&2; exit 1; }

single=""
for f in $files; do
  single="$single --single $f"
done

echo "yodb $("$YODB" --version) against redis $(cat "$REDIS/VERSION") tests"
echo

# --singledb because the external server is one server and the suite otherwise
# expects to be able to move between numbered databases freely between files.
# --dont-clean leaves the logs where a failure can be read.
status=0
# shellcheck disable=SC2086
( cd "$REDIS" && ./runtest --host 127.0.0.1 --port "$PORT" --singledb --dont-clean $single ) || status=$?

exit $status
