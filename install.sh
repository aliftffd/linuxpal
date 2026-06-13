#!/usr/bin/env bash
# Build LinuxPal and install binary + sprites to ~/.local so the Hyprland
# autostart (exec-once in UserConfigs/Startup_Apps.conf) picks up the update.
# Run after any code/asset change:  ./install.sh
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"
ASSET_DIR="$HOME/.local/share/linuxpal/sprites"
STARTUP="$HOME/.config/hypr/UserConfigs/Startup_Apps.conf"
EXEC_LINE='exec-once = env LINUXPAL_ASSETS=$HOME/.local/share/linuxpal/sprites $HOME/.local/bin/linuxpal'

echo "==> building release"
cargo build --release --manifest-path "$REPO/Cargo.toml"

echo "==> installing binaries → $BIN_DIR"
mkdir -p "$BIN_DIR"
install -m755 "$REPO/target/release/linuxpal" "$BIN_DIR/linuxpal"
install -m755 "$REPO/target/release/linuxpal-ctl" "$BIN_DIR/linuxpal-ctl"
install -m755 "$REPO/linuxpal-toggle" "$BIN_DIR/linuxpal-toggle"

echo "==> installing sprites → $ASSET_DIR"
mkdir -p "$ASSET_DIR"
cp -f "$REPO"/assets/sprites/*.png "$ASSET_DIR/"

# add autostart line once, if the Hyprland startup file exists
if [ -f "$STARTUP" ] && ! grep -q "linuxpal" "$STARTUP"; then
  echo "==> adding autostart entry → $STARTUP"
  printf '\n### LinuxPal desktop mascot ###\n%s\n' "$EXEC_LINE" >> "$STARTUP"
fi

echo "==> done. running now (next login it autostarts):"
echo "    env LINUXPAL_ASSETS=$ASSET_DIR $BIN_DIR/linuxpal"
