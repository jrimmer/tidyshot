# Shared by setup.sh and uninstall.sh. Source it; it defines paths and helpers.
tidyshot_data="${XDG_DATA_HOME:-$HOME/.local/share}"
tidyshot_config="${XDG_CONFIG_HOME:-$HOME/.config}"
# Built app files. Kept outside the plugin checkout: the Omarchy plugin
# validator rejects the build outputs a build creates.
tidyshot_runtime="${TIDYSHOT_INSTALL_DIR:-$tidyshot_data/tidyshot-app}"
# Records the plugin commit the runtime was built from; the widget compares it.
tidyshot_stamp="$tidyshot_runtime/.tidyshot-source"
tidyshot_launcher="$HOME/.local/bin/tidyshot"

# The launcher `exec`s the built binary directly (`<runtime>/build/tidyshot`),
# so match that command line, not a Python `-m` invocation.
tidyshot_running() {
  pgrep -u "$(id -u)" -f -- 'build/tidyshot( |$)' >/dev/null 2>&1
}

# Stop a running TidyShot and wait for it, so files are not replaced under it.
# The app has no IPC quit command yet, so terminate each matched process
# directly; never re-run the launcher, which would start a second GUI.
# Returns 1 if it is still running (for example, finishing a recording).
tidyshot_stop() {
  tidyshot_running || return 0
  local task_pid
  for task_pid in $(pgrep -u "$(id -u)" -f -- 'build/tidyshot( |$)' 2>/dev/null); do
    kill -TERM -- "$task_pid" 2>/dev/null || true
  done
  local attempt
  for (( attempt = 0; attempt < ${TIDYSHOT_QUIT_WAIT:-15} * 4; attempt++ )); do
    tidyshot_running || return 0
    sleep 0.25
  done
  return 1
}

# The Hyprland configuration directory TidyShot writes into. Omarchy 4 keeps
# the Lua configuration at the top of ~/.config/hypr; the older layout nested
# it under ~/.config/omarchy/hypr. Prefer the top-level layout and fall back to
# the nested one, so the guard, the writer and `tidyshot doctor` agree on where
# hyprland.lua and bindings.lua live.
tidyshot_hypr_dir() {
  if [[ -f "$tidyshot_config/hypr/hyprland.lua" && -f "$tidyshot_config/hypr/bindings.lua" ]]; then
    printf '%s\n' "$tidyshot_config/hypr"
  elif [[ -f "$tidyshot_config/omarchy/hypr/hyprland.lua" && -f "$tidyshot_config/omarchy/hypr/bindings.lua" ]]; then
    printf '%s\n' "$tidyshot_config/omarchy/hypr"
  fi
}

# The path the installer and launcher write Hyprland bindings to, exported so
# the Python installer and uninstaller use the same layout the guard accepted.
tidyshot_hypr="$(tidyshot_hypr_dir)"
export TIDYSHOT_HYPR_DIR="$tidyshot_hypr"

tidyshot_require_session() {
  if (( EUID == 0 )); then
    printf 'Run this as your desktop user, without sudo.\n' >&2
    return 1
  fi
  if [[ -z $tidyshot_hypr || ! -f "$tidyshot_config/omarchy/shell.json" ]]; then
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
  # Files the checkout owns: tracked plus untracked, minus ignored. Read the
  # NUL-delimited list so newlines in names cannot split an entry, and key an
  # associative array so lookups stay fixed-string, not pattern-based.
  local -A task_owned=()
  while IFS= read -r -d '' task_rel; do
    [[ -n $task_rel ]] && task_owned["$task_rel"]=1
  done < <(git -C "$task_src" -c core.quotePath=false ls-files -z -co --exclude-standard)
  # Drop destination files the source no longer owns. Ignored paths (build
  # outputs, backups) survive because check-ignore matches them here.
  while IFS= read -r -d '' task_path; do
    task_rel=${task_path#"$task_dst"/}
    [[ -z $task_rel ]] && continue
    [[ -L $task_path || -f $task_path ]] || continue
    git -C "$task_src" check-ignore --no-index -q -- "$task_rel" && continue
    [[ -n ${task_owned["$task_rel"]+x} ]] && continue
    rm -f -- "$task_path"
  done < <(find "$task_dst" -mindepth 1 -print0 2>/dev/null)
  # Remove directories that are now empty and not part of the source.
  while IFS= read -r -d '' task_path; do
    task_rel=${task_path#"$task_dst"/}
    [[ -z $task_rel ]] && continue
    git -C "$task_src" check-ignore --no-index -q -- "$task_rel/" && continue
    # Keep a directory that still owns a source file. Compare a fixed prefix
    # so glob characters in plugin paths are never interpreted.
    local task_prefix="$task_rel/" task_has_owner=0
    for task_file in "${!task_owned[@]}"; do
      if [[ $task_file == "$task_prefix"* ]]; then
        task_has_owner=1
        break
      fi
    done
    (( task_has_owner )) && continue
    rmdir -- "$task_path" 2>/dev/null || true
  done < <(find "$task_dst" -mindepth 1 -depth -type d -print0 2>/dev/null)
  # Copy every owned file with `-a`, which preserves symlinks the way
  # `rsync -a` does instead of following them by content.
  for task_rel in "${!task_owned[@]}"; do
    task_file="$task_src/$task_rel"
    [[ -e $task_file || -L $task_file ]] || continue
    mkdir -p -- "$task_dst/$(dirname -- "$task_rel")"
    cp -a -- "$task_file" "$task_dst/$task_rel"
  done
  # `rsync -a` also recreates source directories that hold no owned file, such
  # as an empty tracked tree. Git cannot track those, so enumerate them from
  # the filesystem and skip ignored build outputs.
  while IFS= read -r -d '' task_path; do
    task_rel=${task_path#"$task_src"/}
    [[ -z $task_rel ]] && continue
    git -C "$task_src" check-ignore --no-index -q -- "$task_rel/" && continue
    mkdir -p -- "$task_dst/$task_rel"
  done < <(find "$task_src" -mindepth 1 \( -name .git -prune \) -o -type d -print0 2>/dev/null)
}
