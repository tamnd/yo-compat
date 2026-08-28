#!/bin/sh
# Vendor the real Redis test suite at a pinned version.
#
# Not a rewrite of it, not a subset of it we typed out ourselves, and not a
# description of what we think it checks. It is Redis's own tests/ directory
# from the release tarball, dropped on disk unmodified, because the only
# compatibility claim worth making is the one their tests agree with.
#
# The version is pinned and the tree is thrown away and refetched, so nobody can
# quietly edit a test into passing.
#
# Usage: suite/fetch.sh [--force]

set -eu

HERE="$(cd "$(dirname "$0")/.." && pwd)"
REDIS_VERSION="${REDIS_VERSION:-8.10.1}"
DEST="$HERE/redis"

FORCE=no
for arg in "$@"; do
  case "$arg" in
    --force) FORCE=yes ;;
    *) echo "fetch: no such option: $arg" >&2; exit 2 ;;
  esac
done

if [ "$FORCE" = no ] && [ -f "$DEST/VERSION" ] &&
   [ "$(cat "$DEST/VERSION")" = "$REDIS_VERSION" ]; then
  echo "redis $REDIS_VERSION tests already here"
  exit 0
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "fetching redis $REDIS_VERSION"
curl -fsSL --retry 3 \
  "https://github.com/redis/redis/archive/refs/tags/$REDIS_VERSION.tar.gz" \
  -o "$tmp/redis.tar.gz"

# Only tests/ and the two scripts that drive it. The rest of the tarball is a
# C project we have no use for here, and vendoring it would put a second copy
# of Redis in a repository that is supposed to be about our differences from it.
tar -xzf "$tmp/redis.tar.gz" -C "$tmp" \
  "redis-$REDIS_VERSION/tests" \
  "redis-$REDIS_VERSION/runtest" \
  "redis-$REDIS_VERSION/src/redis-cli.c" 2>/dev/null ||
tar -xzf "$tmp/redis.tar.gz" -C "$tmp" \
  "redis-$REDIS_VERSION/tests" \
  "redis-$REDIS_VERSION/runtest"

rm -rf "$DEST"
mkdir -p "$DEST"
cp -R "$tmp/redis-$REDIS_VERSION/tests" "$DEST/tests"
cp "$tmp/redis-$REDIS_VERSION/runtest" "$DEST/runtest"
chmod +x "$DEST/runtest"
printf '%s\n' "$REDIS_VERSION" > "$DEST/VERSION"

echo "redis $REDIS_VERSION tests are in $DEST"
