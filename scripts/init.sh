#!/usr/bin/env bash
set -euo pipefail

MODULE="${1:-}"
NAME="${2:-}"
SLUG="${3:-}"

if [[ -z "$MODULE" || -z "$NAME" ]]; then
  echo "Usage: make init MODULE=github.com/you/my-app NAME=\"My App\" [SLUG=my-app]" >&2
  exit 1
fi

if [[ -z "$SLUG" ]]; then
  SLUG="$(basename "$MODULE")"
fi

if [[ ! "$SLUG" =~ ^[a-z0-9][a-z0-9_-]*$ ]]; then
  echo "Error: SLUG '$SLUG' must start with [a-z0-9] and contain only lowercase letters, digits, hyphens, or underscores." >&2
  exit 1
fi

CRATE_SLUG="${SLUG//-/_}"
if [[ "$CRATE_SLUG" =~ ^[0-9] ]]; then
  CRATE_SLUG="_${CRATE_SLUG}"
fi

OLD_REPO="robert-crandall/rust-home-server"
OLD_NAME="Rust Home Server"
OLD_SLUG="rust-home-server"
OLD_CRATE="rust_home_server"

NEW_REPO="${MODULE#github.com/}"

echo "Initializing project:"
echo "  Repository : $OLD_REPO -> $NEW_REPO"
echo "  App Name   : $OLD_NAME -> $NAME"
echo "  Slug       : $OLD_SLUG -> $SLUG"
echo "  Crate/DB   : $OLD_CRATE -> $CRATE_SLUG"

FILES=()
while IFS= read -r file; do
  FILES+=("$file")
done < <(git ls-files | grep -Ev '^(scripts/init\.sh|docs/tech-stack\.md)$')

OLD_REPO="$OLD_REPO" NEW_REPO="$NEW_REPO" \
OLD_NAME="$OLD_NAME" NAME="$NAME" \
OLD_SLUG="$OLD_SLUG" SLUG="$SLUG" \
OLD_CRATE="$OLD_CRATE" CRATE_SLUG="$CRATE_SLUG" \
perl -i -pe '
  s/\Q$ENV{OLD_REPO}\E/$ENV{NEW_REPO}/gi;
  s/\Q$ENV{OLD_NAME}\E/$ENV{NAME}/g;
  s/\Q$ENV{OLD_SLUG}\E/$ENV{SLUG}/g;
  s/\Q$ENV{OLD_CRATE}\E/$ENV{CRATE_SLUG}/g;
' "${FILES[@]}"

LEFTOVERS="$(git grep -n -i -E "${OLD_REPO}|${OLD_NAME}|${OLD_SLUG}|${OLD_CRATE}" -- . ':(exclude)scripts/init.sh' ':(exclude)docs/tech-stack.md' || true)"
if [[ -n "$LEFTOVERS" ]]; then
  echo "Error: leftover template references found:" >&2
  echo "$LEFTOVERS" >&2
  exit 1
fi

echo "Done! Next steps:"
echo "  1. Review git diff"
echo "  2. Run: make check && make test"
