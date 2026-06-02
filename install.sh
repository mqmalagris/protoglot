#!/bin/sh
# protoglot CLI installer (Linux / macOS).
#
#   curl -fsSL https://raw.githubusercontent.com/mqmalagris/protoglot/main/install.sh | sh
#
# Downloads the latest release and installs `protoglot` + `pglot` to
# ~/.local/bin (override with PROTOGLOT_BIN_DIR).
set -eu

repo="mqmalagris/protoglot"
bindir="${PROTOGLOT_BIN_DIR:-$HOME/.local/bin}"

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux)
    case "$arch" in
      x86_64) target="x86_64-unknown-linux-gnu" ;;
      *) echo "unsupported Linux arch: $arch" >&2; exit 1 ;;
    esac ;;
  Darwin)
    case "$arch" in
      arm64|aarch64) target="aarch64-apple-darwin" ;;
      x86_64) target="x86_64-apple-darwin" ;;
      *) echo "unsupported macOS arch: $arch" >&2; exit 1 ;;
    esac ;;
  *) echo "unsupported OS: $os" >&2; exit 1 ;;
esac

echo "Looking up the latest protoglot release..."
url="$(curl -fsSL "https://api.github.com/repos/$repo/releases/latest" \
  | grep '"browser_download_url"' | grep "${target}.tar.gz" | head -1 | cut -d '"' -f4)"
if [ -z "$url" ]; then
  echo "no release asset for ${target}" >&2
  exit 1
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $(basename "$url")..."
curl -fsSL "$url" -o "$tmp/protoglot.tar.gz"
tar -xzf "$tmp/protoglot.tar.gz" -C "$tmp"

src="$(dirname "$(find "$tmp" -name protoglot -type f | head -1)")"
mkdir -p "$bindir"
install -m 0755 "$src/protoglot" "$bindir/protoglot"
install -m 0755 "$src/pglot" "$bindir/pglot"

echo "Installed protoglot (and the 'pglot' alias) to $bindir"
case ":$PATH:" in
  *":$bindir:"*) ;;
  *) echo "Note: add $bindir to your PATH." ;;
esac
