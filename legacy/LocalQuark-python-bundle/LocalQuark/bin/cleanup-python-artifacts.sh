#!/usr/bin/env bash
# Stage 12c: physically remove the pre-stub LocalQuark Python / PyInstaller
# artifacts. The .app / .pyc stack is from before 11B-4 (the Rust cookie
# refresher replacement) and before 12a/12b (the in-process TLS launch
# chain). After 12b.2 it has no remaining caller, and the .gitignore
# already excludes it from tracking -- so this script is purely a
# disk-cleanup aid.
#
# Idempotent and dry-run-by-default: pass `--apply` to actually delete.
# The script refuses to touch anything outside this repo (REPO_ROOT) to
# avoid accidents if the user invokes it from the wrong cwd.

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOCALQUARK_REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=lib-common.sh
source "$SCRIPT_DIR/lib-common.sh"

APPLY=0
for arg in "$@"; do
    case "$arg" in
        --apply) APPLY=1 ;;
        -h|--help)
            sed -n '2,18p' "$0"
            exit 0
            ;;
        *)
            echo "usage: $0 [--apply]" >&2
            exit 2
            ;;
    esac
done

# Targets to remove. Each entry is a path relative to LOCALQUARK_REPO_ROOT.
# Order does not matter (rm -rf handles missing paths gracefully).
TARGETS=(
    # Compiled bytecode of retired Python modules. 11B-4 already replaced
    # quark_cookie.__init__.py with a load_cookies()-only stub; the
    # remaining readers (refresher, keychain, sqlite) are gone from the
    # Rust side. These .pyc files are 2-3 months stale and import nothing.
    "app/src/__pycache__/https_proxy.cpython-312.pyc"
    "app/src/__pycache__/https_proxy.cpython-314.pyc"
    "app/src/__pycache__/cache_core.cpython-312.pyc"
    "app/src/__pycache__/cache_core.cpython-314.pyc"

    # Pre-stub PyInstaller output. The .app bundle is the original launcher
    # but its _internal/quark_cookie/ subdir still has pre-11B-4 reader
    # and config pyc. 12b.2 (bash launcher) supersedes it entirely.
    "build/LocalQuark.app"
    "build/LocalQuark"
    "build/dist"
    "build/pyi"

    # Empty directories left over from the stub era. Have contained
    # nothing since the 11B-4 commit; .gitignore already excludes them.
    "app/icons"
    "app/Resources"

    # Stale PyInstaller-era virtualenv. Root .gitignore line 8 already
    # excludes it; the repo never tracks anything inside (~80M on disk).
    ".venv-build"
)

REMOVED_FILES=0
REMOVED_DIRS=0
SKIPPED=0

for rel in "${TARGETS[@]}"; do
    abs="$LOCALQUARK_REPO_ROOT/$rel"
    if [ ! -e "$abs" ]; then
        SKIPPED=$((SKIPPED + 1))
        continue
    fi
    if [ -d "$abs" ]; then
        if [ "$APPLY" -eq 1 ]; then
            rm -rf "$abs"
            REMOVED_DIRS=$((REMOVED_DIRS + 1))
        else
            log "[dry-run] would remove dir  $rel"
            REMOVED_DIRS=$((REMOVED_DIRS + 1))
        fi
    else
        if [ "$APPLY" -eq 1 ]; then
            rm -f "$abs"
            REMOVED_FILES=$((REMOVED_FILES + 1))
        else
            log "[dry-run] would remove file $rel"
            REMOVED_FILES=$((REMOVED_FILES + 1))
        fi
    fi
done

if [ "$APPLY" -eq 0 ]; then
    log "dry-run complete. pass --apply to actually delete."
    log "  files: $REMOVED_FILES, dirs: $REMOVED_DIRS, already-gone: $SKIPPED"
else
    log "cleanup applied. files: $REMOVED_FILES, dirs: $REMOVED_DIRS, already-gone: $SKIPPED"
fi
