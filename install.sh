#!/bin/sh
set -eu

REPO="maxdollinger/annatar"
BIN="annatar"
INSTALL_DIR="${ANNATAR_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${ANNATAR_VERSION:-latest}"

err() {
  echo "error: $*" >&2
  exit 1
}

confirm() {
  ( : </dev/tty ) 2>/dev/null || return 0
  printf '%s' "$1" >/dev/tty
  read -r answer </dev/tty || answer=""
  case "$answer" in
    "" | y | Y | yes | Yes | YES) return 0 ;;
    *) return 1 ;;
  esac
}

need() {
  command -v "$1" >/dev/null 2>&1 || err "'$1' is required but not installed"
}

need curl
need tar
need uname

case "$(uname -s)" in
  Linux) os="unknown-linux-gnu" ;;
  Darwin) os="apple-darwin" ;;
  *) err "unsupported OS: $(uname -s)" ;;
esac

case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  aarch64 | arm64) arch="aarch64" ;;
  *) err "unsupported architecture: $(uname -m)" ;;
esac

if [ "$os" = "apple-darwin" ] && [ "$arch" = "x86_64" ] &&
  [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
  arch="aarch64"
fi

target="$arch-$os"
asset="$BIN-$target.tar.gz"

if [ "$VERSION" = "latest" ]; then
  url="$(curl -fsSLo /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest")" ||
    err "could not look up the latest release"
  case "$url" in
    */tag/*) VERSION="${url##*/tag/}" ;;
    *) err "could not look up the latest release" ;;
  esac
fi
base="https://github.com/$REPO/releases/download/$VERSION"

if [ -x "$INSTALL_DIR/$BIN" ]; then
  installed="$("$INSTALL_DIR/$BIN" --version 2>/dev/null | cut -d ' ' -f 2)" || installed=""
  if [ "$installed" = "${VERSION#v}" ]; then
    echo "$BIN $installed is already installed at $INSTALL_DIR/$BIN."
    exit 0
  fi
  if ! confirm "$BIN ${installed:-(unknown version)} is installed at $INSTALL_DIR/$BIN. Update to ${VERSION#v}? [Y/n] "; then
    echo "Update cancelled."
    exit 0
  fi
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "Downloading $asset ($VERSION)..."
curl -fsSL "$base/$asset" -o "$tmp/$asset" || err "download failed: $base/$asset"
curl -fsSL "$base/$asset.sha256" -o "$tmp/$asset.sha256" || err "checksum download failed"

expected="$(cut -d ' ' -f 1 <"$tmp/$asset.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$asset" | cut -d ' ' -f 1)"
elif command -v shasum >/dev/null 2>&1; then
  actual="$(shasum -a 256 "$tmp/$asset" | cut -d ' ' -f 1)"
else
  err "neither sha256sum nor shasum found, cannot verify download"
fi
[ "$expected" = "$actual" ] || err "checksum mismatch for $asset"

tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$INSTALL_DIR"
mv "$tmp/$BIN-$target/$BIN" "$INSTALL_DIR/$BIN"
chmod +x "$INSTALL_DIR/$BIN"

echo "Installed $BIN to $INSTALL_DIR/$BIN"

case ":$PATH:" in
  *":$INSTALL_DIR:"*)
    echo "Run '$BIN --help' to get started."
    ;;
  *)
    case "${SHELL:-}" in
      */zsh) rc="$HOME/.zshrc" ;;
      */bash) rc="$HOME/.bashrc" ;;
      */fish) rc="" ;;
      *) rc="$HOME/.profile" ;;
    esac
    echo
    echo "warning: $INSTALL_DIR is not in your PATH."
    if [ -n "$rc" ]; then
      echo "Add it by running:"
      echo
      echo "  echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> $rc && . $rc"
    else
      echo "Add it by running:"
      echo
      echo "  fish_add_path $INSTALL_DIR"
    fi
    ;;
esac
