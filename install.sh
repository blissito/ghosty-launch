#!/usr/bin/env bash
# Instalador de Ghosty Launch — baja el binario del último release y lo instala.
#   curl -fsSL https://raw.githubusercontent.com/blissito/ghosty-launch/main/install.sh | sh
set -euo pipefail

REPO="blissito/ghosty-launch"
BIN_DIR="${PREFIX:-$HOME/.local}/bin"

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Darwin)
    case "$arch" in
      arm64) asset="ghosty-launch-macos-arm64" ;;
      x86_64) asset="ghosty-launch-macos-x64" ;;
      *) echo "Arch macOS no soportada: $arch"; exit 1 ;;
    esac ;;
  Linux) asset="ghosty-launch-linux-x64" ;;
  *) echo "OS no soportado ($os). En Windows baja el .zip desde Releases."; exit 1 ;;
esac

url="https://github.com/${REPO}/releases/latest/download/${asset}.tar.gz"
echo "👻 Descargando ${asset}…"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$url" -o "$tmp/g.tar.gz"
tar -xzf "$tmp/g.tar.gz" -C "$tmp"

mkdir -p "$BIN_DIR"
mv "$tmp/ghosty-launch" "$BIN_DIR/ghosty-launch"
chmod +x "$BIN_DIR/ghosty-launch"

# macOS: quita la marca de cuarentena (binario sin notarizar).
if [ "$os" = "Darwin" ]; then
  xattr -dr com.apple.quarantine "$BIN_DIR/ghosty-launch" 2>/dev/null || true
fi

echo "✓ Instalado en $BIN_DIR/ghosty-launch"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "  (próximos runs) agrega a tu PATH:  export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac

# Lánzalo de una vez. Si stdin ya es una terminal (instalación vía
# `sh -c "$(curl …)"`), exec directo. Si no, intenta /dev/tty. En CI, solo avisa.
if [ -t 0 ]; then
  echo "👻 Lanzando…"
  exec "$BIN_DIR/ghosty-launch"
elif [ -e /dev/tty ]; then
  echo "👻 Lanzando…"
  exec "$BIN_DIR/ghosty-launch" </dev/tty
fi
echo "  Corre:  ghosty-launch"
