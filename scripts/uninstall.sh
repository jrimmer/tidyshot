#!/usr/bin/env bash
# Remove TidyShot: shortcuts, window rules, launcher, desktop integration, the
# built app files and the bar widget. Captures, projects and settings are kept.
set -euo pipefail
task_source=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
source "$task_source/scripts/runtime.sh"
task_plugin_id=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["id"])' "$task_source/manifest.json")

fail() {
  printf 'TidyShot uninstall: %s\n' "$*" >&2
  exit 1
}

if (( EUID == 0 )); then fail "run this as your desktop user, without sudo"; fi
if [[ -z ${TIDYSHOT_ASSUME_YES:-} ]]; then
  [[ -t 0 ]] || fail "refusing to continue without confirmation; set TIDYSHOT_ASSUME_YES=1"
  printf 'This removes TidyShot, its shortcuts and its bar widget.\nCaptures and settings in %s are kept.\n' "$tidyshot_data/tidyshot"
  read -r -p 'Remove TidyShot? [y/N] ' task_answer
  [[ $task_answer == [yY]* ]] || fail "cancelled"
fi

tidyshot_stop || fail "TidyShot is still running. Finish or stop any recording, quit TidyShot, then retry."
python3 "$task_source/scripts/uninstall_user.py"
if command -v hyprctl >/dev/null 2>&1 && hyprctl -j version >/dev/null 2>&1; then hyprctl reload >/dev/null; fi

# Only delete app files that setup created; never a git (development) checkout.
if [[ -f $tidyshot_stamp && ! -d $tidyshot_runtime/.git ]]; then
  rm -rf -- "$tidyshot_runtime"
  printf 'Removed %s\n' "$tidyshot_runtime"
fi

task_plugin="$tidyshot_config/omarchy/plugins/$task_plugin_id"
if [[ -d $task_plugin/.git ]]; then
  # Last: this may delete the folder holding this script (bash keeps it open).
  omarchy plugin remove "$task_plugin_id" --yes
fi
printf 'TidyShot has been removed. Captures and settings remain in %s\n' "$tidyshot_data/tidyshot"
