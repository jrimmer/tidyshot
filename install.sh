#!/usr/bin/env bash
# Build TidyShot from a checkout, then install its launchers, desktop and MIME
# entries, Hyprland integration and bar widget. Run as the desktop user,
# without sudo: `omarchy pkg add` handles elevation for system packages.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
task_root="$PWD"
source "$task_root/scripts/runtime.sh"

if (( EUID == 0 )); then
  printf 'Run bash install.sh as your desktop user, without sudo. Use omarchy pkg add for system dependencies.\n' >&2
  exit 1
fi

# Fail before changing user configuration when prerequisites are missing.
task_missing=()
for task_command in cargo qmake6 hyprctl omarchy omarchy-shell grim slurp wl-copy ffmpeg ffprobe tesseract gpu-screen-recorder update-desktop-database update-mime-database xdg-mime notify-send; do
  command -v "$task_command" >/dev/null 2>&1 || task_missing+=("$task_command")
done
if (( ${#task_missing[@]} )); then
  printf 'Missing dependencies: %s\nSee README.md for installation instructions.\n' "${task_missing[*]}" >&2
  exit 1
fi

if [[ ! -f "$tidyshot_config/hypr/hyprland.lua" || ! -f "$tidyshot_config/hypr/bindings.lua" || ! -f "$tidyshot_config/omarchy/shell.json" ]]; then
  printf 'This release requires Omarchy with Lua Hyprland configuration and the shell plugin API.\n' >&2
  exit 1
fi
if [[ -z "${WAYLAND_DISPLAY:-}" || -z "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]] || ! hyprctl -j version >/dev/null 2>&1; then
  printf 'Run the installer inside your running Omarchy desktop session.\n' >&2
  exit 1
fi

# Validate only the shell-facing files; a built checkout contains build
# outputs the plugin validator rejects.
task_plugin_check=$(mktemp -d)
cp -r -- "$task_root/manifest.json" "$task_root/plugin" "$task_plugin_check/"
omarchy plugin validate "$task_plugin_check" || { rm -rf -- "$task_plugin_check"; exit 1; }
rm -rf -- "$task_plugin_check"

cargo build --release --offline
mkdir -p -- "$task_root/build"
cp -p -- "$task_root/target/release/tidyshot" "$task_root/build/tidyshot"

TIDYSHOT_SOURCE="$task_root" python3 "$task_root/scripts/install_user.py"
hyprctl reload
task_config_errors="$(hyprctl configerrors)"
if [[ -n "${task_config_errors//[[:space:]]/}" ]]; then
  printf 'Hyprland reported configuration errors after installation:\n%s\nReview your config and the backup path printed above.\n' "$task_config_errors" >&2
  exit 1
fi
printf 'Hyprland configuration is valid. Launch TidyShot with: tidyshot menu\n'
