#!/usr/bin/env bash
# The plugin manifest is the only source of the release version. Keep the
# parser here so setup and the artifact script cannot disagree about it.

tidyshot_manifest_version() {
  sed -n 's/^[[:space:]]*"version": "\([^"]*\)".*/\1/p' "$1" | head -n1
}
