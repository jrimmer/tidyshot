# Shared by setup.sh and uninstall.sh. Source it; it defines paths and helpers.
tidyshot_data="${XDG_DATA_HOME:-$HOME/.local/share}"
tidyshot_config="${XDG_CONFIG_HOME:-$HOME/.config}"
# Built app files. Kept outside the plugin checkout: the Omarchy plugin
# validator rejects the build outputs a build creates.
tidyshot_runtime="${TIDYSHOT_INSTALL_DIR:-$tidyshot_data/tidyshot-app}"
# Records the plugin commit the runtime was built from; the widget compares it.
tidyshot_stamp="$tidyshot_runtime/.tidyshot-source"
tidyshot_launcher="$HOME/.local/bin/tidyshot"

tidyshot_running() {
  pgrep -u "$(id -u)" -f -- '-m tidyshot( |$)' >/dev/null 2>&1
}

# Quit a running TidyShot and wait for it, so files are not replaced under it.
# Returns 1 if it is still running (for example, finishing a recording).
tidyshot_stop() {
  tidyshot_running || return 0
  [[ -x $tidyshot_launcher ]] && "$tidyshot_launcher" quit >/dev/null 2>&1 || true
  local attempt
  for (( attempt = 0; attempt < ${TIDYSHOT_QUIT_WAIT:-15} * 4; attempt++ )); do
    tidyshot_running || return 0
    sleep 0.25
  done
  return 1
}

tidyshot_require_session() {
  if (( EUID == 0 )); then
    printf 'Run this as your desktop user, without sudo.\n' >&2
    return 1
  fi
  if [[ ! -f "$tidyshot_config/hypr/hyprland.lua" || ! -f "$tidyshot_config/hypr/bindings.lua" || ! -f "$tidyshot_config/omarchy/shell.json" ]]; then
    printf 'TidyShot requires Omarchy with Lua Hyprland configuration and the shell plugin API.\n' >&2
    return 1
  fi
  if [[ -z "${WAYLAND_DISPLAY:-}" || -z "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]] || ! hyprctl -j version >/dev/null 2>&1; then
    printf 'Run this in a terminal inside your running Omarchy desktop session.\n' >&2
    return 1
  fi
}

# Copy the plugin checkout into the runtime, dropping removed files but
# keeping everything the checkout's .gitignore protects: build outputs, the
# cargo target directory, and configuration backups. This mirrors `rsync -a
# --delete --exclude=/.git --filter=':- .gitignore'` without requiring rsync.
tidyshot_mirror() {
  local task_src=$1 task_dst=$2 task_rel task_path task_file
  mkdir -p -- "$task_dst"
  # Files the checkout owns: tracked plus untracked, minus ignored.
  local task_list
  task_list=$(git -C "$task_src" ls-files -co --exclude-standard)
  # Drop destination files the source no longer owns. Ignored paths (build
  # outputs, backups) survive because check-ignore matches them here.
  while IFS= read -r -d '' task_path; do
    task_rel=${task_path#"$task_dst"/}
    [[ -z $task_rel ]] && continue
    [[ -L $task_path || -f $task_path ]] || continue
    git -C "$task_src" check-ignore --no-index -q -- "$task_rel" && continue
    if ! printf '%s\n' "$task_list" | grep -qxF -- "$task_rel"; then
      rm -f -- "$task_path"
    fi
  done < <(find "$task_dst" -mindepth 1 -print0 2>/dev/null)
  # Remove directories that are now empty and not part of the source.
  while IFS= read -r -d '' task_path; do
    task_rel=${task_path#"$task_dst"/}
    [[ -z $task_rel ]] && continue
    git -C "$task_src" check-ignore --no-index -q -- "$task_rel/" && continue
    # Keep a directory that still owns a source file. A fixed substring match
    # avoids regex metacharacters in plugin paths.
    printf '%s\n' "$task_list" | grep -qF -- "$task_rel/" && continue
    rmdir -- "$task_path" 2>/dev/null || true
  done < <(find "$task_dst" -mindepth 1 -depth -type d -print0 2>/dev/null)
  # Copy every owned file, preserving content and mode.
  while IFS= read -r task_rel; do
    [[ -z $task_rel ]] && continue
    task_file="$task_src/$task_rel"
    [[ -e $task_file ]] || continue
    mkdir -p -- "$task_dst/$(dirname -- "$task_rel")"
    cp -p -- "$task_file" "$task_dst/$task_rel"
  done <<<"$task_list"
}
