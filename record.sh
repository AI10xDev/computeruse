#!/usr/bin/env bash

set -euo pipefail

if (( $# < 2 )); then
  echo "Usage: $0 OUTPUT.mp4 GUI_INSTRUCTIONS_FILE [AGENT_OPTIONS...]" >&2
  exit 2
fi

output=$1
instructions=$2
shift 2
project_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
computeruse_bin=${COMPUTERUSE_BIN:-computeruse}
credentials_file=${COMPUTERUSE_CREDENTIALS:-$project_dir/ast}

if [[ -z "${AZURE_OPENAI_ENDPOINT:-}" && -z "${AZURE_OPENAI_API_KEY:-}" && -z "${OPENAI_API_KEY:-}" && -f "$credentials_file" ]]; then
  mapfile -t credentials < "$credentials_file"
  if [[ -n "${credentials[0]:-}" && -n "${credentials[2]:-}" ]]; then
    export AZURE_OPENAI_API_KEY=${credentials[0]%$'\r'}
    export AZURE_OPENAI_ENDPOINT=${credentials[2]%$'\r'}
  fi
fi

if [[ -z "${COMPUTERUSE_BIN:-}" && -x "$project_dir/target/release/computeruse" ]]; then
  computeruse_bin="$project_dir/target/release/computeruse"
fi

if [[ ! -f "$instructions" ]]; then
  echo "Instructions file not found: $instructions" >&2
  exit 1
fi

if [[ -e "$output" ]]; then
  echo "Output already exists: $output" >&2
  exit 1
fi

for command in ffmpeg xdpyinfo "$computeruse_bin"; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Required command not found: $command" >&2
    exit 1
  fi
done

display=${DISPLAY:-:0.0}
size=$(xdpyinfo -display "$display" | awk '/dimensions:/{print $2; exit}')
frames=$(mktemp -d "${TMPDIR:-/tmp}/computeruse-frames.XXXXXX")
ffmpeg_pid=
pruner_pid=

cleanup() {
  trap - EXIT INT TERM
  if [[ -n "$ffmpeg_pid" ]] && kill -0 "$ffmpeg_pid" 2>/dev/null; then
    kill -INT "$ffmpeg_pid" 2>/dev/null || true
    wait "$ffmpeg_pid" 2>/dev/null || true
  fi
  if [[ -n "$pruner_pid" ]] && kill -0 "$pruner_pid" 2>/dev/null; then
    kill "$pruner_pid" 2>/dev/null || true
    wait "$pruner_pid" 2>/dev/null || true
  fi
  rm -rf -- "$frames"
}
trap cleanup EXIT INT TERM

prune_frames() {
  shopt -s nullglob
  while kill -0 "$ffmpeg_pid" 2>/dev/null; do
    local frame_files=("$frames"/frame-*.png)
    local excess=$((${#frame_files[@]} - 20))
    if (( excess > 0 )); then
      rm -f -- "${frame_files[@]:0:excess}"
    fi
    sleep 1
  done
}

if [[ -z "$size" ]]; then
  echo "Could not determine screen dimensions for display $display" >&2
  exit 1
fi

execute=false
for argument in "$@"; do
  if [[ "$argument" == "--execute" ]]; then
    execute=true
    break
  fi
done

if [[ "$execute" == false ]]; then
  echo "Agent is in dry-run mode; add --execute to enable mouse and keyboard input." >&2
fi

echo "Recording $display to $output and sampling agent observations at 2 FPS."

ffmpeg \
  -nostdin \
  -f x11grab \
  -framerate 30 \
  -video_size "$size" \
  -i "$display" \
  -map 0:v \
  -c:v libx264 \
  -preset veryfast \
  "$output" \
  -map 0:v \
  -vf fps=2 \
  "$frames/frame-%09d.png" &
ffmpeg_pid=$!
prune_frames &
pruner_pid=$!

"$computeruse_bin" agent \
  --frames "$frames" \
  --instructions "$instructions" \
  "$@"
