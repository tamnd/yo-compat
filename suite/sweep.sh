#!/bin/sh
# Run every Redis test file on its own and print what each one did.
#
# suite/runtest.sh runs the files it is given in one go, which is what you want
# once a file passes and never want while you are finding out whether it does.
# The suite aborts the whole run on the first exception, and an exception is the
# normal way a file ends when it reaches a command from a milestone we have not
# started, so one run over forty files tells you about the first four of them.
#
# This runs each file in its own process with its own server and its own time
# limit, so one file that hangs costs one time limit and not the sweep. What
# comes out is three numbers per file: tests that passed, tests that failed, and
# whether the file ended early. A file with a pass count and nothing else is a
# candidate for suite/passing.txt.
#
# The default list is everything under unit/ that a server started outside the
# suite can run at all. The module tests need the suite to load a .so into a
# server it started itself, and the cluster and tls tests need it to start
# several with options we do not read, so none of them are a statement about
# our compatibility either way and they are left out rather than counted as
# failures.
#
# Usage: suite/sweep.sh [--timeout SECONDS] [--all] [file ...]
#   --all        include the module, cluster and tls files
#   --timeout N  how long one file gets before it is killed, default 90

set -eu

HERE="$(cd "$(dirname "$0")/.." && pwd)"
YODB="${YODB:-$HERE/../yo/target/release/yodb}"
PORT="${PORT:-7512}"
REDIS="$HERE/redis"
OUT="${OUT:-$HERE/results/sweep}"

[ -d "$REDIS/tests" ] || { echo "run suite/fetch.sh first" >&2; exit 2; }
[ -x "$YODB" ] || { echo "no yodb at $YODB. Set YODB." >&2; exit 2; }

LIMIT=90
ALL=no
files=""
while [ $# -gt 0 ]; do
  case "$1" in
    --all) ALL=yes ;;
    --timeout) shift; LIMIT="$1" ;;
    -*) echo "sweep: no such option: $1" >&2; exit 2 ;;
    *) files="$files $1" ;;
  esac
  shift
done

if [ -z "$files" ]; then
  files="$(cd "$REDIS/tests" && find unit -name '*.tcl' | sed 's|\.tcl$||' | sort)"
  if [ "$ALL" = no ]; then
    files="$(printf '%s\n' "$files" | grep -v '^unit/moduleapi/' | grep -v '^unit/cluster/' | grep -v '^unit/tls$')"
  fi
fi

mkdir -p "$OUT"
echo "yodb $("$YODB" --version) against redis $(cat "$REDIS/VERSION") tests, ${LIMIT}s per file"
echo
printf '%-34s %6s %6s  %s\n' file ok err ended

clean=""
for f in $files; do
  log="$OUT/$(printf '%s' "$f" | tr / -).log"

  "$YODB" serve --port "$PORT" >"$OUT/server.log" 2>&1 &
  server=$!
  i=0
  while [ $i -lt 100 ]; do
    if printf 'PING\r\n' | timeout 1 nc -q1 127.0.0.1 "$PORT" 2>/dev/null | grep -q PONG; then
      break
    fi
    i=$((i + 1))
    sleep 0.05
  done
  if [ $i -ge 100 ]; then
    kill $server 2>/dev/null || true
    printf '%-34s %6s %6s  %s\n' "$f" - - "server never answered"
    continue
  fi

  status=0
  ( cd "$REDIS" && timeout "$LIMIT" ./runtest --host 127.0.0.1 --port "$PORT" \
      --singledb --dont-clean --single "$f" ) >"$log" 2>&1 || status=$?
  kill $server 2>/dev/null || true
  wait $server 2>/dev/null || true

  ok=$(grep -c '^\[ok\]' "$log" || true)
  err=$(grep -c '^\[err\]' "$log" || true)
  if [ "$status" = 124 ]; then
    ended="timed out after ${LIMIT}s"
  elif grep -q '^\[exception\]' "$log"; then
    # The first line of the exception says which command it died on, which is
    # nearly always a command from a later milestone.
    ended="$(grep -m1 '^\[exception\]' "$log" | sed 's/^\[exception\]: Executing test client: //' | cut -c1-46)"
  elif [ "$err" != 0 ]; then
    ended="failures"
  elif [ "$status" != 0 ]; then
    ended="runtest exited $status"
  elif [ "$ok" = 0 ]; then
    # Every test in the file was skipped, which nearly always means the file is
    # tagged for a server the suite started itself. It ran to the end and it
    # said nothing about us, so it is not a pass.
    ended="nothing ran"
  else
    ended="clean"
    clean="$clean $f"
  fi
  printf '%-34s %6s %6s  %s\n' "$f" "$ok" "$err" "$ended"
done

echo
echo "clean:${clean:- none}"
echo "logs in $OUT"
