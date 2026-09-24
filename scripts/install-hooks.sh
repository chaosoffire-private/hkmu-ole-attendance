#!/usr/bin/env bash
#
# Install the repository's git hooks.
#
# Hooks are not tracked by git, so they cannot be committed; this script is the
# committable source of truth. Run it once per clone:
#
#     scripts/install-hooks.sh
#
# It points `core.hooksPath` at .githooks/, so every clone that runs it shares
# the same hooks and there is nothing to copy into .git/hooks by hand.

set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

chmod +x .githooks/pre-commit .githooks/pre-push scripts/scan-secrets.sh
git config core.hooksPath .githooks

echo "hooks installed: core.hooksPath = .githooks"
echo "  pre-commit  scans the staged index"
echo "  pre-push    scans the commits being pushed"
