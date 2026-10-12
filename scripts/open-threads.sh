#!/usr/bin/env bash
# Open or focus the threads pane in the invoking workspace, across all its tabs.
# Same placement rules as open-board.sh: scoped lookup, tab focus before pane focus,
# split anchored to the invoking pane.
set -uo pipefail

herdr_bin="${HERDR_BIN_PATH:-herdr}"
workspace_id="${HERDR_WORKSPACE_ID:-}"
target_pane="${HERDR_PANE_ID:-}"
origin_tab="${HERDR_TAB_ID:-}"
if [ -z "$workspace_id" ] || [ -z "$target_pane" ] || [ -z "$origin_tab" ]; then
  printf 'tsk: cannot open threads without a Herdr workspace, tab and pane\n' >&2
  exit 1
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
plugin_bin="${TSK_BIN:-$script_dir/../target/release/tsk}"
if [ ! -x "$plugin_bin" ]; then
  printf 'tsk: threads launcher binary is unavailable: %s\n' "$plugin_bin" >&2
  exit 1
fi

if ! panes="$("$herdr_bin" pane list --workspace "$workspace_id")"; then
  printf 'tsk: could not list panes in workspace %s\n' "$workspace_id" >&2
  exit 1
fi
found="$(printf '%s' "$panes" | "$plugin_bin" threads --find-pane || true)"
if [ -n "$found" ]; then
  pane_id="${found%%$'\t'*}"
  tab_id="${found#*$'\t'}"
  if ! "$herdr_bin" tab focus "$tab_id"; then
    exit 1
  fi
  if "$herdr_bin" plugin pane focus "$pane_id"; then
    exit 0
  fi
fi

if ! "$herdr_bin" tab focus "$origin_tab"; then
  exit 1
fi
exec "$herdr_bin" plugin pane open --plugin herdr-tsk --entrypoint threads --placement split --target-pane "$target_pane" --focus
