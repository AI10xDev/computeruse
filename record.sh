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
acceleration=${COMPUTERUSE_VIDEO_ACCEL:-auto}
explicit_vaapi_device=${COMPUTERUSE_VAAPI_DEVICE:-}

case "$acceleration" in
  auto|vaapi|off) ;;
  *)
    echo "Invalid COMPUTERUSE_VIDEO_ACCEL value '$acceleration'; expected auto, vaapi, or off." >&2
    exit 2
    ;;
esac

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

for command in ffmpeg xdpyinfo timeout "$computeruse_bin"; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Required command not found: $command" >&2
    exit 1
  fi
done

display=${DISPLAY:-:0.0}
size=$(xdpyinfo -display "$display" | awk '/dimensions:/{print $2; exit}')
if [[ -z "$size" ]]; then
  echo "Could not determine screen dimensions for display $display" >&2
  exit 1
fi

frames=$(mktemp -d "${TMPDIR:-/tmp}/computeruse-frames.XXXXXX")
ffmpeg_pid=
pruner_pid=
agent_pid=

stop_child() {
  local pid=${1:-}
  local signal=${2:-TERM}
  local attempt
  if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
    kill "-$signal" "$pid" 2>/dev/null || true
    for (( attempt = 0; attempt < 50; attempt++ )); do
      if ! kill -0 "$pid" 2>/dev/null; then
        break
      fi
      sleep 0.1
    done
    if kill -0 "$pid" 2>/dev/null; then
      kill -TERM "$pid" 2>/dev/null || true
      sleep 0.2
    fi
    if kill -0 "$pid" 2>/dev/null; then
      kill -KILL "$pid" 2>/dev/null || true
    fi
  fi
  if [[ -n "$pid" ]]; then
    wait "$pid" 2>/dev/null || true
  fi
}

cleanup() {
  trap - EXIT
  stop_child "$agent_pid" TERM
  stop_child "$ffmpeg_pid" INT
  stop_child "$pruner_pid" TERM
  rm -rf -- "$frames"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

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

vaapi_device=
vaapi_diagnostic=
probe_vaapi() {
  local device=$1
  local diagnostic
  if [[ ! -r "$device" || ! -w "$device" ]]; then
    vaapi_diagnostic="cannot read and write VA-API render node $device"
    return 1
  fi
  if diagnostic=$(timeout 10 ffmpeg \
      -hide_banner -loglevel error -nostdin \
      -vaapi_device "$device" \
      -f lavfi -i "color=c=black:s=$size:r=30" \
      -frames:v 5 -vf format=nv12,hwupload \
      -c:v h264_vaapi -qp 23 -f null - 2>&1); then
    vaapi_device=$device
    return 0
  fi
  vaapi_diagnostic="${diagnostic:-FFmpeg VA-API probe failed without a diagnostic}"
  return 1
}

echo "Requested video acceleration mode: $acceleration" >&2
if [[ "$acceleration" != off ]]; then
  if [[ -n "$explicit_vaapi_device" ]]; then
    candidates=("$explicit_vaapi_device")
  else
    shopt -s nullglob
    candidates=(/dev/dri/renderD*)
    shopt -u nullglob
  fi

  probe_failures=()
  for candidate in "${candidates[@]}"; do
    if probe_vaapi "$candidate"; then
      break
    fi
    probe_failures+=("$candidate: $vaapi_diagnostic")
    if [[ -n "$explicit_vaapi_device" ]]; then
      break
    fi
  done

  if [[ -z "$vaapi_device" ]]; then
    if (( ${#candidates[@]} == 0 )); then
      probe_failures+=("no accessible /dev/dri/renderD* devices found")
    fi
    if [[ "$acceleration" == vaapi ]]; then
      printf 'VA-API recording is required but no device passed preflight:\n' >&2
      printf '  %s\n' "${probe_failures[@]}" >&2
      exit 1
    fi
    printf 'VA-API recording unavailable; falling back to software encoding:\n' >&2
    printf '  %s\n' "${probe_failures[@]}" >&2
  else
    echo "Selected VA-API device: $vaapi_device" >&2
  fi
else
  echo "Using software video encoding without probing VA-API devices." >&2
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
ffmpeg_args=(
  -hide_banner
  -nostdin
  -n
)
if [[ -n "$vaapi_device" ]]; then
  ffmpeg_args+=(
    -vaapi_device "$vaapi_device"
  )
fi
ffmpeg_args+=(
  -f x11grab
  -framerate 30
  -video_size "$size"
  -i "$display"
  -map 0:v
)
if [[ -n "$vaapi_device" ]]; then
  ffmpeg_args+=(
    -vf format=nv12,hwupload
    -c:v h264_vaapi
    -qp 23
  )
else
  ffmpeg_args+=(
    -c:v libx264
    -preset veryfast
    -pix_fmt yuv420p
  )
fi
ffmpeg_args+=(
  -g 60
  -movflags +empty_moov+default_base_moof+frag_keyframe
  -flush_packets 1
  "$output"
  -map 0:v
  -vf fps=2
  -atomic_writing 1
  "$frames/frame-%09d.png"
)

ffmpeg "${ffmpeg_args[@]}" &
ffmpeg_pid=$!

ready=false
for (( attempt = 0; attempt < 100; attempt++ )); do
  if ! kill -0 "$ffmpeg_pid" 2>/dev/null; then
    if wait "$ffmpeg_pid"; then
      ffmpeg_status=0
    else
      ffmpeg_status=$?
    fi
    ffmpeg_pid=
    echo "FFmpeg exited with status $ffmpeg_status before producing the first complete observation." >&2
    exit 1
  fi
  for frame in "$frames"/frame-*.png; do
    if [[ -s "$frame" ]]; then
      ready=true
      break 2
    fi
  done
  sleep 0.1
done
if [[ "$ready" != true ]]; then
  echo "Timed out waiting for FFmpeg to produce the first observation." >&2
  exit 1
fi

prune_frames &
pruner_pid=$!
"$computeruse_bin" agent \
  --frames "$frames" \
  --instructions "$instructions" \
  "$@" &
agent_pid=$!

if wait -n -p completed_pid "$agent_pid" "$ffmpeg_pid"; then
  completed_status=0
else
  completed_status=$?
fi

if [[ "$completed_pid" == "$ffmpeg_pid" ]]; then
  ffmpeg_pid=
  stop_child "$agent_pid" TERM
  agent_pid=
  echo "FFmpeg exited with status $completed_status while the agent was running; the partial recording was retained." >&2
  exit 1
fi

agent_status=$completed_status
agent_pid=
stop_child "$ffmpeg_pid" INT
ffmpeg_pid=
stop_child "$pruner_pid" TERM
pruner_pid=

if (( agent_status != 0 )); then
  echo "Agent exited with status $agent_status; the recording was finalized." >&2
  exit "$agent_status"
fi
if [[ ! -s "$output" ]]; then
  echo "FFmpeg did not produce a usable recording at $output." >&2
  exit 1
fi
