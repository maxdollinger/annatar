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
  base="https://github.com/$REPO/releases/latest/download"
else
  base="https://github.com/$REPO/releases/download/$VERSION"
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
