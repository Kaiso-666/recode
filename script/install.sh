#!/usr/bin/env sh
# Install Re:Code on Linux or macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/Kaiso-666/recode/main/script/install.sh | sh
#
# Prefers a release binary; falls back to `cargo install` when run from a
# checkout.
set -eu

REPO="Kaiso-666/recode"
VERSION="${RECODE_VERSION:-latest}"
BIN_DIR="${RECODE_BIN_DIR:-$HOME/.local/bin}"

os="$(uname -s | tr '[:upper:]' '[:lower:]')"
case "$os" in
  linux) os="linux" ;;
  darwin) os="macos" ;;
  *) echo "unsupported operating system: $os" >&2; exit 1 ;;
esac

arch="$(uname -m)"
case "$arch" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="aarch64" ;;
  *) echo "unsupported architecture: $arch" >&2; exit 1 ;;
esac

asset="recode-$os-$arch"
mkdir -p "$BIN_DIR"

url="https://github.com/$REPO/releases/latest/download/$asset"
if [ "$VERSION" != "latest" ]; then
  url="https://github.com/$REPO/releases/download/$VERSION/$asset"
fi

if curl -fsSL "$url" -o "$BIN_DIR/recode.tmp" 2>/dev/null; then
  mv "$BIN_DIR/recode.tmp" "$BIN_DIR/recode"
  chmod +x "$BIN_DIR/recode"
else
  echo "release download failed; trying cargo install" >&2
  root="${RECODE_ROOT:-$HOME/.local}"
  cargo install --path crates/recode-cli --root "$root" --locked
  if [ "$root/bin/recode" != "$BIN_DIR/recode" ]; then
    cp "$root/bin/recode" "$BIN_DIR/recode"
  fi
fi

"$BIN_DIR/recode" --version
echo "Installed to $BIN_DIR/recode"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "Add $BIN_DIR to PATH to run 'recode'." ;;
esac
