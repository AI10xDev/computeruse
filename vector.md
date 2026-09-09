# Integrated GPU Acceleration Plan

## Goal

Use FFmpeg's VA-API support to offload supported video encoding and decoding to
an Intel or AMD integrated GPU, while retaining a usable software path on
machines without compatible hardware, drivers, or permissions.

This uses the GPU's video engine, not general-purpose GPU vectorization. It
does not add Rust compute shaders, accelerate mouse arithmetic, or move the
remote vision model onto the local machine. No new Rust GPU bindings are needed.

This document is a plan only; acceleration has not been implemented or verified
on the current machine.

## Current Paths

- `record.sh`: captures X11 at 30 FPS, encodes with `libx264`, and emits PNG
  observations at 2 FPS from the same FFmpeg process.
- `src/video.rs::extract_video_frames`: invokes FFmpeg without hardware options
  and writes every decoded frame as an ordered PNG.
- `src/bin/video-frames.rs` and `src/main.rs::run_agent`: call that extraction
  function. Live-agent mode reads PNG files and does not decode video itself.
- `Cargo.toml`: has no GPU compute dependency. Keep it that way for this work.

## Configuration

Use the same environment variables in the recording script and Rust extraction
path, avoiding changes to existing command arguments and public signatures:

| Variable | Behavior |
| --- | --- |
| `COMPUTERUSE_VIDEO_ACCEL=auto` | Default. Attempt supported VA-API processing; fall back to software with a diagnostic. |
| `COMPUTERUSE_VIDEO_ACCEL=vaapi` | Require VA-API; return an actionable error rather than silently using software. |
| `COMPUTERUSE_VIDEO_ACCEL=off` | Use the existing software path without probing GPU devices. |
| `COMPUTERUSE_VAAPI_DEVICE=/dev/dri/renderD128` | Optional explicit render node. Do not silently substitute a different GPU. |

Reject invalid modes. Without an explicit device, inspect accessible
`/dev/dri/renderD*` nodes in deterministic order and validate candidates with
FFmpeg. A render-node number does not identify an integrated GPU: on multi-GPU
systems, document how to identify and explicitly select the intended adapter.
Log the selected device, requested acceleration mode, and any software fallback
to stderr.

Do not automatically install drivers, change device permissions, require root,
or assume that an advertised FFmpeg encoder proves the hardware works.

## Implementation

### 1. Establish A Baseline

- Check the installed FFmpeg build for VA-API decode support and `h264_vaapi`.
- Inspect DRM render nodes and their adapter identities. Use `vainfo` if
  available to check driver profiles and entry points; do not require it at
  runtime.
- Measure software recording CPU usage and dropped frames at the target screen
  size, plus extraction time for a representative video.
- Keep hardware checks optional so development and CI still work without a GPU.

### 2. Accelerate Recording

Update `record.sh` while preserving its two outputs and existing agent options.

- Before starting the agent or creating the requested recording, run a short,
  timeout-bounded synthetic encode to a null output using the selected device,
  actual screen dimensions, pixel format, and intended encoder settings.
- In `auto`, try usable candidates and choose `libx264 -preset veryfast` if
  none pass. In `vaapi`, fail with the FFmpeg diagnostic and device context.
- For the accelerated recording output, initialize the VA-API device and use
  `-vf format=nv12,hwupload -c:v h264_vaapi`. Choose and document an explicit
  quality setting after testing; do not reuse x264-only options such as its
  preset or assume matching quality numbers produce equivalent images.
- Scope the upload filter and hardware encoder to the MP4 output only. Keep
  the observation output mapped from the original software frames with
  `-vf fps=2`, avoiding an unnecessary GPU upload/download for PNG observations.
- Preserve 30 FPS capture, 2 FPS observations, frame pruning, output
  no-overwrite behavior, and clean MP4 finalization on exit. Add explicit FFmpeg
  no-overwrite handling rather than relying only on the initial file check.
- Do not silently crop or resize screens whose dimensions the encoder cannot
  support. Use software in `auto`, or report the limitation in `vaapi`.
- Confirm capture readiness with a completed observation and a live FFmpeg
  process before launching the agent, with a bounded startup timeout.
- Monitor FFmpeg while the agent runs. If capture fails, stop the agent and
  return a recording error instead of letting it operate on stale frames.
- Limit automatic recording fallback to preflight. A startup or mid-recording
  failure must be reported, not hidden by restarting capture and losing part
  of the session. Finalize or retain the partial recording where possible.

X11 capture, color conversion, and PNG encoding still consume CPU. This is not
a zero-copy desktop capture pipeline.

### 3. Accelerate Offline Decoding

Update `src/video.rs` so both extraction callers benefit without duplicating
FFmpeg configuration in the CLIs.

- In hardware modes, supply `-hwaccel vaapi` and `-hwaccel_device DEVICE` before
  `-i`. Decoder support must be verified against the actual input codec,
  profile, and bit depth; an H.264 encoding probe is not a decoding test.
- Keep decoded frames CPU-accessible for the PNG encoder. If hardware-frame
  output is explicitly requested, add the required download and pixel-format
  conversion rather than assuming every input can be downloaded as NV12.
- Preserve all-frame extraction, ordering, numbering from zero, and refusal
  to use an output directory containing existing numbered frames.
- In `auto`, if hardware decoding fails, remove only numbered frames produced
  by that attempt and retry once with software decoding. Require cleanup to
  succeed before retrying so stale files cannot mix with the new sequence.
- In `vaapi`, clean partial output and report failure without a software retry.
  If the software retry also fails, retain useful diagnostics from both attempts.
- Keep unrelated output-directory files intact. Missing FFmpeg, invalid input,
  zero decoded frames, and unwritable output must remain clear errors.

Hardware decoding may not improve end-to-end extraction: downloading frames,
PNG compression, and disk writes remain costs. Measure before claiming a speedup.

### 4. Add Regression Coverage

Use fake FFmpeg, display-query, and agent executables for deterministic tests.
Pass `PATH` and configuration to child processes rather than changing the
process-global environment in parallel Rust tests. Never call a live model API
or inject input during these tests.

- Cover `auto`, `vaapi`, `off`, invalid modes, explicit device selection, absent
  devices, failed probes, and forced-mode errors.
- Check that hardware input options precede `-i`, and that recording-only
  filters and encoder options do not leak into the PNG output.
- Simulate partially successful decoding followed by failure. Verify cleanup,
  one software retry, ordered results, and preservation of unrelated files.
- Verify existing recordings and extracted frames are never overwritten.
- Test capture startup failure, capture death while the agent runs, agent exit,
  and interruption. Ensure child processes and temporary frames are cleaned up.
- Keep real VA-API tests opt-in and skip them clearly when hardware is absent.

### 5. Document And Verify

Update `README.md` and `QUICKSTART.md` with the environment variables, software
override, render-node permissions, driver requirements, multi-GPU selection,
and the distinction between video acceleration and local model inference.

Run the repository's standard checks plus shell validation:

```bash
bash -n record.sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo doc --no-deps
```

Run new script tests and ShellCheck if available. On a supported machine,
exercise a short recording with a stub agent, inspect the MP4 with `ffprobe`,
and compare software and hardware extraction frame counts and ordering. Do not
require pixel-identical decoder output across backends. Include unsupported
codec/profile cases to verify fallback and strict-mode errors.

Compare recording CPU usage, dropped frames, file size, visual quality, and
extraction elapsed time against the baseline. Confirm video-engine activity
with an appropriate vendor tool where available; codec metadata alone does not
prove the intended adapter was used. No fixed performance gain is promised.

## Acceptance Criteria

- Supported, explicitly selected integrated GPUs perform VA-API H.264 recording
  and supported offline decoding.
- Machines without usable VA-API retain software operation in `auto` and `off`.
- Forced hardware mode fails clearly instead of claiming acceleration while
  using software.
- Agent observation cadence, recording finalization, frame ordering, and
  no-overwrite safeguards are preserved.
- Tests run without GPU hardware, display access, API credentials, or input
  device privileges; hardware validation results are reported separately.
- Changes remain limited to the video paths, focused tests, and documentation.
  Local inference and general GPU vectorization remain out of scope.
