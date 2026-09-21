#!/bin/sh
# maskrun installer (Linux, macOS)
#
#   curl -fsSL https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.sh | sh
#
# Options, as environment variables (a piped script cannot prompt):
#   MASKRUN_BIN=~/bin          where to install      (default: ~/.local/bin)
#   MASKRUN_REF=v0.1.0         git ref to fetch      (default: main)
#   MASKRUN_WITH_GUARD=1       also register the Claude Code guard hook
#
# Installs one file. To remove it: rm "$MASKRUN_BIN/maskrun"
set -eu

REPO="furkanakyol/maskrun"
REF="${MASKRUN_REF:-main}"
BIN_DIR="${MASKRUN_BIN:-$HOME/.local/bin}"
SOURCE_URL="https://raw.githubusercontent.com/$REPO/$REF/bin/maskrun"
TARGET="$BIN_DIR/maskrun"

say()  { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

# --- python ----------------------------------------------------------------
PYTHON=""
for candidate in python3 python; do
    if command -v "$candidate" >/dev/null 2>&1; then
        if "$candidate" -c 'import sys; sys.exit(0 if sys.version_info >= (3, 9) else 1)' 2>/dev/null; then
            PYTHON="$candidate"
            break
        fi
    fi
done
[ -n "$PYTHON" ] || die "Python 3.9 or newer is required but was not found on PATH.
  Debian/Ubuntu: apt install python3
  Fedora:        dnf install python3
  macOS:         already present, or 'brew install python'"

# --- keyring prerequisites -------------------------------------------------
OS="$(uname -s)"
case "$OS" in
    Darwin)
        command -v security >/dev/null 2>&1 \
            || warn "/usr/bin/security not found; the macOS keychain backend will not work."
        ;;
    Linux)
        if ! command -v secret-tool >/dev/null 2>&1; then
            warn "secret-tool (libsecret) not found — maskrun needs it to reach the keyring.
  Debian/Ubuntu: apt install libsecret-tools
  Fedora:        dnf install libsecret
  Arch:          pacman -S libsecret
  You also need a running Secret Service (gnome-keyring, KWallet's Secret Service, or KeePassXC)."
        fi
        ;;
    *)
        warn "unrecognised system '$OS'. maskrun supports Linux, macOS and Windows.
  On Windows use install.ps1 instead."
        ;;
esac

# --- download --------------------------------------------------------------
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT INT TERM

if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$SOURCE_URL" -o "$TMP" || die "download failed: $SOURCE_URL"
elif command -v wget >/dev/null 2>&1; then
    wget -qO "$TMP" "$SOURCE_URL" || die "download failed: $SOURCE_URL"
else
    die "neither curl nor wget is available"
fi

# A truncated or error-page download must never be installed.
head -n 1 "$TMP" | grep -q '^#!/usr/bin/env python3' \
    || die "downloaded file does not look like maskrun (wrong ref '$REF'?)"
"$PYTHON" -c "import ast,sys; ast.parse(open(sys.argv[1]).read())" "$TMP" \
    || die "downloaded file is not valid Python — refusing to install it"

mkdir -p "$BIN_DIR"
cat "$TMP" > "$TARGET"
chmod 755 "$TARGET"

VERSION="$("$PYTHON" "$TARGET" --version 2>/dev/null || echo 'maskrun (version unknown)')"
say "installed $VERSION -> $TARGET"

# --- PATH ------------------------------------------------------------------
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        say ""
        warn "$BIN_DIR is not on your PATH. Add this to your shell profile:"
        say "    export PATH=\"$BIN_DIR:\$PATH\""
        ;;
esac

# --- optional guard --------------------------------------------------------
if [ "${MASKRUN_WITH_GUARD:-0}" = "1" ]; then
    say ""
    "$PYTHON" "$TARGET" install-guard --command-path "$TARGET" || \
        warn "could not register the guard hook; do it later with: maskrun install-guard"
fi

say ""
say "Next:"
say "  maskrun put myapp-database-url      store a secret (prompts, not echoed)"
say "  maskrun import .env --dry-run       see what would move out of a .env"
say "  maskrun run -- npm run dev          run with secrets injected, output masked"
say ""
say "Using an AI coding agent? Register the guard so it cannot print your secrets:"
say "  maskrun install-guard"
