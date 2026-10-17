#!/usr/bin/env bash
# Build and install TidyShot from its Omarchy plugin checkout
# (~/.config/omarchy/plugins/io.github.joshdaws.tidyshot). The bar widget runs
# this for first-time setup and after `omarchy plugin update`; rerunning it is safe.
set -euo pipefail
task_source=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
source "$task_source/scripts/runtime.sh"

fail() {
  printf 'TidyShot setup: %s\n' "$*" >&2
  exit 1
}

tidyshot_require_session || exit 1
[[ -d $task_source/.git ]] ||
  fail "run this from the Omarchy plugin checkout. For a development checkout, use: bash install.sh"
task_commit=$(git -C "$task_source" rev-parse HEAD) || fail "could not read the plugin revision"

# Check the app folder before changing anything.
task_legacy=0
if [[ -e $tidyshot_runtime || -L $tidyshot_runtime ]] && [[ ! -f $tidyshot_stamp ]]; then
  if [[ -d $tidyshot_runtime/.git ]]; then
    task_legacy=1
  elif [[ -L $tidyshot_runtime || ! -d $tidyshot_runtime || -n $(find "$tidyshot_runtime" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null) ]]; then
    fail "$tidyshot_runtime exists and was not created by TidyShot setup. Move it aside, then retry."
  fi
fi

if [[ -z ${TIDYSHOT_SKIP_PACKAGES:-} ]]; then
  printf 'Installing missing system dependencies through Omarchy. You may be asked for your password.\n'
  # `rust` provides cargo; `qt6-base`/`qt6-declarative` provide the Qt headers,
  # libraries and QML build tools that install.sh and bin/build need.
  omarchy pkg add git rust qt6-base qt6-declarative python gcc pkgconf \
    wayland wayland-protocols \
    curl cairo libxkbcommon libglvnd lua54 grim slurp wl-clipboard ffmpeg \
    tesseract tesseract-data-eng tesseract-data-osd zbar gpu-screen-recorder libpulse \
    desktop-file-utils shared-mime-info xdg-utils libnotify
fi

task_was_running=0
if tidyshot_running; then
  task_was_running=1
  printf 'Quitting TidyShot so it can be updated.\n'
  tidyshot_stop || fail "TidyShot is still running. Finish or stop any recording, quit TidyShot, then retry."
fi

if (( task_legacy )); then
  # An install from before plugin support: keep it, including its config
  # backups, but move it out of the way. Captures live elsewhere.
  task_previous="$tidyshot_runtime.pre-plugin-$(date +%Y%m%d-%H%M%S)"
  mv -- "$tidyshot_runtime" "$task_previous"
  printf 'Moved the previous TidyShot checkout to %s\nDelete it once you are happy with this installation.\n' "$task_previous"
fi

task_previous_build=""
[[ -f $tidyshot_stamp ]] && task_previous_build=$(<"$tidyshot_stamp")

# A release tarball is used only when the checkout is exactly the released
# commit, the host can run the published x86_64 binary, and its checksum
# matches. Everything else builds from source, as before. The release is staged
# outside the runtime so a failed download, checksum or archive leaves the
# installed app untouched.
task_release_stage=""
task_release_reason=""
task_release_archive=""
task_release_version=""
task_release_tmp=""
task_release_cleanup() {
  [[ -n $task_release_tmp ]] && rm -rf -- "$task_release_tmp"
}
trap task_release_cleanup EXIT

task_release_source() {
  local task_name=$1 task_dest=$2
  if [[ -n ${TIDYSHOT_RELEASE_DIR:-} ]]; then
    [[ -f $TIDYSHOT_RELEASE_DIR/$task_name ]] || return 1
    cp -a -- "$TIDYSHOT_RELEASE_DIR/$task_name" "$task_dest"
    return 0
  fi
  local task_base=${TIDYSHOT_RELEASE_URL:-https://github.com/jrimmer/tidyshot/releases/download}
  command -v curl >/dev/null 2>&1 || return 1
  curl -fsSL --proto '=https' --connect-timeout 5 --max-time 300 \
    --output "$task_dest" "$task_base/v$task_release_version/$task_name"
}

task_prepare_release() {
  task_release_version=$(sed -n 's/^[[:space:]]*"version": "\\([^"\]*\\)".*/\1/p' "$task_source/manifest.json" | head -n1)
  [[ -n $task_release_version ]] || {
    task_release_reason="the plugin manifest has no version"
    return 0
  }
  local task_arch=${TIDYSHOT_UNAME:-$(uname -m)}
  if [[ $task_arch != x86_64 ]]; then
    task_release_reason="no prebuilt binary for $task_arch"
    return 0
  fi
  local task_archive="tidyshot-$task_release_version-x86_64.tar.gz"
  task_release_tmp=$(mktemp -d)
  local task_checksums="$task_release_tmp/checksums.txt"
  local task_tarball="$task_release_tmp/$task_archive"
  if ! task_release_source checksums.txt "$task_checksums"; then
    task_release_reason="release checksums are unavailable"
    return 0
  fi
  if ! task_release_source "$task_archive" "$task_tarball"; then
    task_release_reason="release tarball $task_archive is unavailable"
    return 0
  fi
  local task_expected
  task_expected=$(awk -v name="$task_archive" '$2 == name || $2 == "*" name { print $1; exit }' "$task_checksums")
  if [[ ! $task_expected =~ ^[0-9a-fA-F]{64}$ ]]; then
    task_release_reason="release checksums do not name $task_archive"
    return 0
  fi
  local task_actual
  task_actual=$(sha256sum -- "$task_tarball" | awk '{ print $1 }')
  if [[ ${task_expected,,} != "$task_actual" ]]; then
    task_release_reason="release checksum mismatch for $task_archive"
    return 0
  fi
  task_release_stage="$task_release_tmp/unpacked"
  mkdir -p -- "$task_release_stage"
  if ! tar -xzf "$task_tarball" -C "$task_release_stage" --no-same-owner --no-same-permissions; then
    task_release_reason="release tarball $task_archive could not be unpacked"
    return 0
  fi
  local task_released_commit=""
  [[ -f $task_release_stage/.tidyshot-source ]] &&
    task_released_commit=$(head -n1 "$task_release_stage/.tidyshot-source")
  if [[ -z $task_released_commit ]]; then
    task_release_reason="release tarball $task_archive has no source commit"
    return 0
  fi
  if [[ $task_released_commit != "$task_commit" ]]; then
    task_release_reason="release $task_release_version was built from ${task_released_commit:0:12}, not the checkout HEAD ${task_commit:0:12}"
    return 0
  fi
  if [[ ! -x $task_release_stage/build/tidyshot ]]; then
    task_release_reason="release tarball $task_archive has no executable"
    return 0
  fi
  task_release_archive="$task_archive"
}

task_prepare_release
if [[ -n $task_release_reason ]]; then
  printf 'TidyShot setup: not using the prebuilt release: %s; building from source.\n' "$task_release_reason" >&2
fi

mkdir -p -- "$tidyshot_runtime"
# Remove the stamp first so an interrupted setup reads as unfinished.
rm -f -- "$tidyshot_stamp"
tidyshot_mirror "$task_source" "$tidyshot_runtime"
if [[ -n $task_release_stage ]]; then
  mkdir -p -- "$tidyshot_runtime/build"
  cp -a -- "$task_release_stage/build/." "$tidyshot_runtime/build/"
  printf 'TidyShot setup: installing the prebuilt %s release.\n' "$task_release_archive"
  (cd -- "$tidyshot_runtime" && TIDYSHOT_PREBUILT=1 bash install.sh)
else
  (cd -- "$tidyshot_runtime" && bash install.sh)
fi
printf '%s\n' "$task_commit" >"$tidyshot_stamp"

# The shell hot-reloads a changed plugin but can keep serving the previous
# widget component from its cache, so restart it when the widget changed
# since the last build. An unknown previous commit counts as changed.
if [[ -n $task_previous_build && $task_previous_build != "$task_commit" ]] &&
  ! git -C "$task_source" diff --quiet "$task_previous_build" "$task_commit" -- manifest.json plugin 2>/dev/null; then
  printf 'Restarting the Omarchy shell to load the updated bar widget.\n'
  omarchy restart shell || true
fi

if (( task_was_running )); then
  setsid -f "$tidyshot_launcher" >/dev/null 2>&1 </dev/null || true
fi
printf '\nTidyShot is ready. Use the bar icon, press Print, or run: tidyshot menu\n'
