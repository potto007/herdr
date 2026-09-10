#!/usr/bin/env bash
# Add the PATH/ZIG exports and the `herdr-update` alias that `just install-handoff`
# needs to ~/.zshrc or ~/.bashrc. Idempotent: skips lines already present.
# Usage: scripts/setup_local_build_shell.sh [rcfile]
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
rc="${1:-}"
if [ -z "$rc" ]; then
    case "$(basename "${SHELL:-}")" in
        zsh) rc="$HOME/.zshrc" ;;
        bash) rc="$HOME/.bashrc" ;;
        *) echo "unknown shell; pass the rc file explicitly" >&2; exit 1 ;;
    esac
fi

lines=(
    '# herdr local build: rustup via Homebrew, Zig 0.15 for vendored libghostty-vt'
    'export PATH="/opt/homebrew/opt/rustup/bin:$PATH"'
    'export ZIG=/opt/homebrew/opt/zig@0.15/bin/zig'
    "alias herdr-update='just -f $repo/justfile -d $repo install-handoff'"
)

touch "$rc"
added=0
for line in "${lines[@]}"; do
    if ! grep -qxF -- "$line" "$rc"; then
        [ "$added" -eq 0 ] && printf '\n' >> "$rc"
        printf '%s\n' "$line" >> "$rc"
        added=$((added + 1))
    fi
done
echo "added $added line(s) to $rc; open a new shell or: source $rc"
