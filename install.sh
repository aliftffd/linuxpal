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

echo "==> checking dependencies"
if pkg-config --exists xkbcommon; then
  echo "==> xkbcommon found on the system"
  LOCAL_PKG_CONFIG_PATH=""
else
  echo "==> xkbcommon not found by pkg-config. Trying to find system runtime library..."
  XKB_SO=""
  for PATH_TO_CHECK in \
    /usr/lib/x86_64-linux-gnu/libxkbcommon.so.0 \
    /usr/lib/libxkbcommon.so.0 \
    /usr/lib64/libxkbcommon.so.0 \
    /lib/x86_64-linux-gnu/libxkbcommon.so.0 \
    /usr/lib/aarch64-linux-gnu/libxkbcommon.so.0 \
    /usr/lib/arm-linux-gnueabihf/libxkbcommon.so.0; do
    if [ -f "$PATH_TO_CHECK" ]; then
      XKB_SO="$PATH_TO_CHECK"
      break
    fi
  done

  if [ -n "$XKB_SO" ]; then
    echo "==> Found system library at $XKB_SO. Setting up local pkgconfig fallback in target/..."
    LOCAL_PKGCONFIG="$REPO/target/local-pkgconfig"
    mkdir -p "$LOCAL_PKGCONFIG/lib"
    
    # Create dynamic symlink
    ln -sf "$XKB_SO" "$LOCAL_PKGCONFIG/lib/libxkbcommon.so"
    
    # Write the temporary pc file
    cat <<EOF > "$LOCAL_PKGCONFIG/xkbcommon.pc"
prefix=/usr
exec_prefix=\${prefix}
libdir=$LOCAL_PKGCONFIG/lib
includedir=\${prefix}/include

Name: xkbcommon
Description: XKB API common parts (dynamically generated local fallback)
Version: 1.6.0
Libs: -L\${libdir} -lxkbcommon
Cflags: -I\${includedir}
EOF
    LOCAL_PKG_CONFIG_PATH="$LOCAL_PKGCONFIG"
  else
    echo "==> ERROR: libxkbcommon.so.0 not found in common locations."
    echo "    Please install libxkbcommon development files using your package manager:"
    echo "    - Debian/Ubuntu: sudo apt install libxkbcommon-dev"
    echo "    - Arch Linux:    sudo pacman -S libxkbcommon"
    echo "    - Fedora:        sudo dnf install libxkbcommon-devel"
    exit 1
  fi
fi

echo "==> building release"
if [ -n "$LOCAL_PKG_CONFIG_PATH" ]; then
  PKG_CONFIG_PATH="$LOCAL_PKG_CONFIG_PATH" cargo build --release --manifest-path "$REPO/Cargo.toml"
else
  cargo build --release --manifest-path "$REPO/Cargo.toml"
fi

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

# For non-Hyprland (e.g., Mutter/GNOME), create a .desktop autostart entry
AUTOSTART_DIR="$HOME/.config/autostart"
if [ -d "$AUTOSTART_DIR" ] || [ ! -f "$STARTUP" ]; then
  echo "==> adding autostart entry → $AUTOSTART_DIR/linuxpal.desktop"
  mkdir -p "$AUTOSTART_DIR"
  cat <<EOF > "$AUTOSTART_DIR/linuxpal.desktop"
[Desktop Entry]
Type=Application
Name=LinuxPal
Comment=Pixel-art desktop mascot
Exec=env LINUXPAL_ASSETS=$HOME/.local/share/linuxpal/sprites $HOME/.local/bin/linuxpal
Icon=linuxpal
Terminal=false
Categories=Utility;
X-GNOME-Autostart-enabled=true
EOF
  chmod +x "$AUTOSTART_DIR/linuxpal.desktop"
fi

echo "==> done. running now (next login it autostarts):"
echo "    env LINUXPAL_ASSETS=$ASSET_DIR $BIN_DIR/linuxpal"
