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

# Fail before changing user configuration when prerequisites are missing. A
# prebuilt install needs no Rust or Qt toolchain: it only rebuilds the
# auto-scroll helper, which is plain C against the running compositor.
task_missing=()
task_commands=(hyprctl omarchy omarchy-shell grim slurp wl-copy ffmpeg ffprobe tesseract
  zbarimg gpu-screen-recorder update-desktop-database update-mime-database xdg-mime notify-send)
if [[ -n ${TIDYSHOT_PREBUILT:-} ]]; then
  task_commands+=(gcc pkg-config wayland-scanner)
else
  task_commands+=(cargo)
fi
for task_command in "${task_commands[@]}"; do
  command -v "$task_command" >/dev/null 2>&1 || task_missing+=("$task_command")
done
# cxx-qt-build finds Qt through the `QMAKE` override or a `qmake`/`qmake6`
# binary on PATH; do not insist on the distro-specific `qmake6` name.
if [[ -z ${TIDYSHOT_PREBUILT:-} && -z ${QMAKE:-} ]] &&
  ! command -v qmake6 >/dev/null 2>&1 && ! command -v qmake >/dev/null 2>&1; then
  task_missing+=(qmake6)
fi
if (( ${#task_missing[@]} )); then
  printf 'Missing dependencies: %s\nSee README.md for installation instructions.\n' "${task_missing[*]}" >&2
  exit 1
fi

if [[ -z $tidyshot_hypr || ! -f "$tidyshot_config/omarchy/shell.json" ]]; then
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

# scripts/setup.sh stages a release binary in build/. It already carries the
# executable, so skip the cargo build but never the local scroll helper: that
# helper targets the running compositor's Wayland protocol.
if [[ -n ${TIDYSHOT_PREBUILT:-} ]]; then
  if [[ ! -x $task_root/build/tidyshot ]]; then
    printf 'The prebuilt TidyShot binary is missing from %s/build.\n' "$task_root" >&2
    exit 1
  fi
  bash "$task_root/bin/native"
  for task_helper in scroll-helper window-helper; do
    if [[ ! -x $task_root/native/$task_helper ]]; then
      printf 'The prebuilt TidyShot install has no %s; install wayland-scanner and wayland-client headers.\n' "$task_helper" >&2
      exit 1
    fi
  done
  mkdir -p -- "$task_root/build/native"
  cp -- "$task_root/native/scroll-helper" "$task_root/native/window-helper" "$task_root/build/native/"
  # generate-icon comes from the same release as build/tidyshot: scripts/setup.sh
  # only stages a tarball whose sha256 matches checksums.txt and whose members
  # pass its path, link and special-file checks, so running it here carries the
  # same provenance as the binary.
  if [[ -x $task_root/build/generate-icon && -n ${HOME:-} ]]; then
    "$task_root/build/generate-icon" "$task_root/build/tidyshot.svg" >/dev/null
  fi
  printf 'installed prebuilt build/tidyshot with a locally built auto-scroll helper\n' >&2
else
  bash "$task_root/bin/build"
fi

TIDYSHOT_SOURCE="$task_root" python3 "$task_root/scripts/install_user.py"
hyprctl reload
task_config_errors="$(hyprctl configerrors)"
if [[ -n "${task_config_errors//[[:space:]]/}" ]]; then
  printf 'Hyprland reported configuration errors after installation:\n%s\nReview your config and the backup path printed above.\n' "$task_config_errors" >&2
  exit 1
fi
printf 'Hyprland configuration is valid. Launch TidyShot with: tidyshot menu\n'
