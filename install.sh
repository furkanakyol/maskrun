#!/bin/sh
# maskrun installer (Linux, macOS)
#
#   curl -fsSL https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.sh | sh
#
# Options, as environment variables (a piped script cannot prompt):
#   MASKRUN_BIN=~/bin          where to install        (default: ~/.local/bin)
#   MASKRUN_VERSION=v0.1.0     release to install       (default: latest)
#   MASKRUN_WITH_GUARD=1       also register the Claude Code guard hook
#   MASKRUN_BASE_URL=...       release base URL, for testing against a mirror
#                              (default: https://github.com/furkanakyol/maskrun/releases)
#
# Installs one binary. To remove it: rm "$MASKRUN_BIN/maskrun"
set -eu

REPO="furkanakyol/maskrun"
BIN_DIR="${MASKRUN_BIN:-$HOME/.local/bin}"
BASE_URL="${MASKRUN_BASE_URL:-https://github.com/$REPO/releases}"
API_URL="https://api.github.com/repos/$REPO/releases/latest"

say()  { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$2" "$1"
    else
        die "neither curl nor wget is available"
    fi
}

OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS" in
    Linux)  os_part=unknown-linux-gnu ;;
    Darwin) os_part=apple-darwin ;;
    *) die "unsupported OS '$OS'. maskrun ships Linux, macOS and Windows builds.
  Windows: use install.ps1 instead.
  Anything else: cargo install maskrun" ;;
esac
case "$ARCH" in
    x86_64|amd64)  arch_part=x86_64 ;;
    arm64|aarch64) arch_part=aarch64 ;;
    *) die "unsupported architecture '$ARCH'.
  Build from source instead: cargo install maskrun" ;;
esac
TARGET="${arch_part}-${os_part}"

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT INT TERM

VERSION="${MASKRUN_VERSION:-}"
if [ -z "$VERSION" ]; then
    fetch "$API_URL" "$WORKDIR/latest.json" || die "could not reach $API_URL to find the latest version"
    VERSION="$(grep '"tag_name"' "$WORKDIR/latest.json" | head -n1 | sed -E 's/.*"tag_name":[[:space:]]*"([^"]+)".*/\1/')"
    [ -n "$VERSION" ] || die "could not parse a version out of $API_URL"
fi

ASSET="maskrun-${VERSION}-${TARGET}.tar.gz"
ARCHIVE="$WORKDIR/$ASSET"
SUMS="$WORKDIR/SHA256SUMS"

fetch "$BASE_URL/download/$VERSION/$ASSET" "$ARCHIVE" \
    || die "download failed: $BASE_URL/download/$VERSION/$ASSET"
fetch "$BASE_URL/download/$VERSION/SHA256SUMS" "$SUMS" \
    || die "download failed: $BASE_URL/download/$VERSION/SHA256SUMS"

# The one thing standing between a tampered or truncated download and a
# secret manager landing on disk. Not optional, not warn-and-continue.
EXPECTED="$(awk -v f="$ASSET" '{fn=$2; sub(/^\*/, "", fn); if (fn == f) print $1}' "$SUMS")"
[ -n "$EXPECTED" ] || die "SHA256SUMS has no entry for $ASSET"

if command -v sha256sum >/dev/null 2>&1; then
    ACTUAL="$(sha256sum "$ARCHIVE" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
    ACTUAL="$(shasum -a 256 "$ARCHIVE" | awk '{print $1}')"
else
    die "neither sha256sum nor shasum is available to verify the download"
fi

[ "$EXPECTED" = "$ACTUAL" ] || die "checksum mismatch for $ASSET
  expected: $EXPECTED
  actual:   $ACTUAL
The download is corrupted or was tampered with. Not installing."

tar -xzf "$ARCHIVE" -C "$WORKDIR" maskrun
mkdir -p "$BIN_DIR"
install -m 755 "$WORKDIR/maskrun" "$BIN_DIR/maskrun"

VERSION_OUTPUT="$("$BIN_DIR/maskrun" --version 2>/dev/null)" \
    || die "installed but did not run: $BIN_DIR/maskrun --version"
say "installed $VERSION_OUTPUT -> $BIN_DIR/maskrun"

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        say ""
        warn "$BIN_DIR is not on your PATH. Add this to your shell profile:"
        say "    export PATH=\"$BIN_DIR:\$PATH\""
        ;;
esac

if [ "${MASKRUN_WITH_GUARD:-0}" = "1" ]; then
    say ""
    "$BIN_DIR/maskrun" install-guard || \
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
