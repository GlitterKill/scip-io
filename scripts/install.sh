#!/usr/bin/env sh
# SCIP-IO CLI installer
#
# Usage:
#   curl -LsSf https://github.com/GlitterKill/scip-io/releases/latest/download/install.sh | sh
#   curl -LsSf https://github.com/GlitterKill/scip-io/releases/download/v0.1.0/install.sh | sh
#
# Environment variables:
#   SCIP_IO_VERSION        — tag to install (default: latest)
#   SCIP_IO_INSTALL_DIR    — install location (default: $HOME/.local/bin)

set -eu

REPO="GlitterKill/scip-io"
VERSION="${SCIP_IO_VERSION:-latest}"
INSTALL_DIR="${SCIP_IO_INSTALL_DIR:-$HOME/.local/bin}"

# ---------- helpers ----------
err() { printf "\033[1;31merror:\033[0m %s\n" "$1" >&2; exit 1; }
info() { printf "\033[1;36m==>\033[0m %s\n" "$1"; }
ok()   { printf "\033[1;32m ok\033[0m %s\n" "$1"; }

need_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        err "required command '$1' not found on PATH"
    fi
}

need_cmd curl
need_cmd tar
need_cmd uname
need_cmd mktemp

# ---------- detect platform ----------
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$OS-$ARCH" in
    darwin-x86_64)  TARGET="x86_64-apple-darwin" ;;
    darwin-arm64)   TARGET="aarch64-apple-darwin" ;;
    linux-x86_64)   TARGET="x86_64-unknown-linux-gnu" ;;
    linux-aarch64)  err "linux-aarch64 is not yet published. Please build from source." ;;
    *) err "unsupported platform: $OS-$ARCH" ;;
esac

info "Detected platform: $TARGET"

# ---------- resolve version ----------
if [ "$VERSION" = "latest" ]; then
    info "Resolving latest release..."
    VERSION="$(curl -LsSf "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep -o '"tag_name": *"[^"]*"' \
        | head -n1 \
        | sed 's/.*"\([^"]*\)"$/\1/')"
    if [ -z "$VERSION" ]; then
        err "could not resolve latest release tag"
    fi
fi

info "Installing scip-io $VERSION"

# ---------- download ----------
ARCHIVE="scip-io-${VERSION}-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE}"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

info "Downloading $URL"
if ! curl -LsSf -o "$TMP/$ARCHIVE" "$URL"; then
    err "failed to download $URL"
fi

info "Extracting..."
tar -xzf "$TMP/$ARCHIVE" -C "$TMP"

# ---------- install ----------
mkdir -p "$INSTALL_DIR"
BIN_SRC="$TMP/scip-io-${VERSION}-${TARGET}/scip-io"
BIN_DST="$INSTALL_DIR/scip-io"

if [ ! -f "$BIN_SRC" ]; then
    err "expected binary not found in archive: $BIN_SRC"
fi

mv "$BIN_SRC" "$BIN_DST"
chmod +x "$BIN_DST"

ok "Installed to $BIN_DST"

# ---------- PATH hint ----------
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        printf "\n\033[1;33mnote:\033[0m %s is not on your PATH.\n" "$INSTALL_DIR"
        printf "Add this line to your shell config (~/.bashrc, ~/.zshrc, etc):\n\n"
        printf "    export PATH=\"%s:\$PATH\"\n\n" "$INSTALL_DIR"
        ;;
esac

printf "\nRun '\033[1;36mscip-io --help\033[0m' to get started.\n"
