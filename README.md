# computeruse

**Vision-guided Linux desktop control with GPT-6-Astra.**

`computeruse` is a Rust command-line tool and library that connects visual
reasoning to real mouse and keyboard input. Give it a task in a text file and
desktop screenshots: GPT-6-Astra interprets the screen, selects GUI actions,
and checks subsequent observations for progress. Use it to interact with
applications, evaluate decisions against recorded sessions, or build your own
computer-use workflow.

The default model identifier is `gpt-6-astra`. Model inference runs through a
remote API; screenshot handling, action validation, and input execution run
locally. The agent prints decisions without controlling the desktop unless
you explicitly pass `--execute`.

The recommended live workflow uses native X11 on Linux, including Ubuntu 26.04.
Lower-level input control also supports Wayland through `evdev` and `uinput`,
but verified cursor positioning and the included screen-recording script
require X11.

## How Computer Use Works

1. **Observe:** capture the desktop with `record.sh`, watch an existing image
   directory, or sample frames from a prerecorded video.
2. **Decide:** send the task, a bounded screenshot history, and previous
   transitions to GPT-6-Astra for the next batch of typed actions.
3. **Act:** validate actions and optionally execute mouse movement, clicks,
   scrolling, key taps, hotkeys, or key sequences locally.
4. **Check:** in live execution, wait for fresh post-action screenshots before
   making the next decision. Save JSONL transitions to inspect actions,
   rationale, model-reported progress, and cursor feedback.

This is a screenshot-driven feedback loop, not a fixed macro or a continuous
video stream to the model. Each decision makes a separate network request.

## Features

- Turn natural-language GUI instructions into actions using GPT-6-Astra vision.
- Run a live observe/act loop or evaluate decisions against prerecorded video.
- Batch visible-target movement and clicks, or type key sequences, without a
  model round trip between every local input event.
- Verify cursor arrival with proportional X11 control before a following click.
- Start in dry-run mode and explicitly opt into desktop input with `--execute`.
- Record sessions and export JSONL traces for debugging and offline evaluation.
- Capture, inject, record, and replay mouse and keyboard events independently
  of the model through the CLI or reusable Rust library.

## Requirements

- Linux with `evdev` and `uinput` enabled (standard in Ubuntu kernels).
- A native X11 session for `record.sh` and the agent's default verified cursor
  control. Mouse record/play calibration also needs a screen at least 961 by
  541 pixels.
- Rust 1.88 or newer to build from source.
- FFmpeg on `PATH` for video frame extraction and visual-agent processing.
- `xdpyinfo` and `timeout` on `PATH` when using `record.sh`.
- API credentials and an endpoint exposing `gpt-6-astra`, or a compatible
  vision model selected with `--model`.
- Read access to mouse devices under `/dev/input` for capture.
- Write access to `/dev/uinput` for control.

Ubuntu does not grant these permissions to every process by default. For a
quick local test, run the command with `sudo`. For regular use, prefer a
dedicated group:

```bash
sudo groupadd --force input
sudo usermod --append --groups input "$USER"
printf '%s\n' 'KERNEL=="uinput", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"' \
  | sudo tee /etc/udev/rules.d/80-computeruse.rules
sudo modprobe uinput
sudo udevadm control --reload-rules
sudo udevadm trigger --name-match=uinput
```

Log out and back in after changing group membership. Membership in `input`
allows observing and injecting input for the local system, so grant it only to
trusted users and services.

## Install

For a short setup walkthrough, see [`QUICKSTART.md`](QUICKSTART.md).

```bash
cargo install --git https://github.com/AI10xDev/computeruse
```

To build a local checkout:

```bash
cargo build --release
./target/release/computeruse --help
```

## GPT-6-Astra Setup

Configure an Azure Foundry project endpoint or Azure OpenAI endpoint:

```bash
export AZURE_OPENAI_API_KEY=...
export AZURE_OPENAI_ENDPOINT=https://RESOURCE.services.ai.azure.com/api/projects/PROJECT
```

The Azure configuration uses the Responses API. When `AZURE_OPENAI_ENDPOINT`
is unset, the agent instead uses `OPENAI_API_KEY` and an optional
`OPENAI_BASE_URL` with the Chat Completions API. The endpoint must support image
inputs and the agent's JSON-output request format.

`gpt-6-astra` is the configured default, not a guarantee that every endpoint
exposes that model name. Pass `--model YOUR_DEPLOYMENT_NAME` if your endpoint
uses a different name. No model weights run locally.

## Start A Computer-Use Session

From a local checkout, create an instruction file and preview the agent's
decisions while recording the desktop:

```bash
printf '%s\n' 'Open the browser settings page.' > gui-steps.txt

# Safe default: capture the screen and print decisions, without sending input.
./record.sh ./preview.mp4 ./gui-steps.txt --model gpt-6-astra

# Execute model-selected actions and save a decision trace.
./record.sh ./session.mp4 ./gui-steps.txt \
  --model gpt-6-astra \
  --execute \
  --trace ./trajectory.jsonl
```

Each output video path must be new. The script runs FFmpeg and the agent
together, then finalizes the recording when the agent exits. Extra arguments
are forwarded to `computeruse agent`; specifying the default model above is
optional.

Screenshots are sent to the configured remote endpoint even in dry-run mode.
Use a test desktop without sensitive information, and supervise execution:
`--execute` sends real input to the active desktop, not a sandbox.

## Agent Functionality

The `agent` command evaluates GUI instructions from a text file against either
a live frame directory supplied through `--frames` or a recorded video supplied
through `--frame`. In live mode it waits for new PNG, JPEG, or WebP images after
each decision, providing the state-estimation feedback loop needed to observe
the result of executed actions. Each policy call receives the latest bounded
trajectory and prior transitions, then emits typed mouse, keyboard, scroll, or
wait actions. The transition log includes the policy's progress estimate and
its change as a reward signal for evaluation or offline reinforcement learning.

For typing into a focused field, the policy can emit a `key_sequence` action
with 1 to 256 named keys or hotkeys. The entire sequence is validated before any
actions execute, then each entry is pressed and released once locally without
a model request or screenshot wait between letters. Intentional doubled letters
are preserved. For example:

```json
{"type":"key_sequence","keys":["h","e","l","l","o","dot","shift+a"]}
```

These are physical keys on the active keyboard layout, not Unicode text input.
The agent reports frame-wait, policy-request, local input/action, frame-discard,
and total decision-cycle durations separately on stderr, along with raw image
bytes (before base64 encoding). Input/action time includes explicit `wait`
actions; policy time includes request preparation and response parsing. This
distinguishes model latency from capture delays and slow event emission. When
instructions explicitly request typing a known URL or query and pressing Enter, the policy is instructed to put
`enter` last in the same key sequence, avoiding an unnecessary model round trip.

For visible click targets, the policy is instructed to batch `mouse_move_to`
and `mouse_click` in one decision. Cursor verification and the click run locally
without a model request or screenshot wait between them. Hover-dependent targets
still require an observation before clicking, and navigation/focus changes must
be observed before choosing new targets or typing. Routine fixed waits are
discouraged. The remote vision policy still requires a network request per
decision; these changes remove unnecessary round trips, not model inference.

### Proportional Cursor Control

With `--execute`, the default `--mouse-control proportional` executes
`mouse_move_to` through native X11 pixel warps and pointer queries, not a single
unverified uinput jump. It converts normalized coordinates to the current X11
root's pixel bounds, uses a proportional gain of 0.5 with short-horizon velocity
damping, and limits each correction to 96 pixels. Steps shrink near the target.
Observations are spaced by at least 16 ms; movement returns only after two
consecutive readings are within 2 pixels. Failure stops the action batch before
a following click or key sequence. The polling budget includes nominal travel
time plus two seconds of settling allowance; it is not an X11 I/O timeout.

`cursor_motion` in JSONL transitions contains one report per controlled target
move, in action order: start/target/observed positions, residual pixels,
correction count, elapsed milliseconds, final velocity, and peak measured speed.
The local estimator uses actual pixel displacement and monotonic timestamps,
resets velocity after gaps over 250 ms, and caps extrapolation at 100 ms. These
reports also reach the next policy decision. They measure cursor movement, not
visual target motion or whether the application accepted a click.

This mode requires a native X11 session, working `DISPLAY`/Xauthority, and
full-desktop frames matching that display's selected screen. Cropped frames or
frames from another display do not share its coordinate mapping. The connected
server's `XFree86-DGA` extension identifies Xorg even when `XDG_SESSION_TYPE` is
missing or stale (for example, XFCE on Xorg with a `wayland` session label).
Otherwise, `XDG_SESSION_TYPE=x11` is required. A server advertising `XWAYLAND` is
always rejected: XWayland does not provide reliable global feedback for native
Wayland windows. Absence of that extension alone is not treated as proof of X11.
If detection is inconclusive, only set `XDG_SESSION_TYPE=x11` for the command
after confirming that `DISPLAY` targets native Xorg, not merely an XFCE desktop.
Use `--mouse-control direct` explicitly for the previous unverified uinput path,
including on Wayland. Dry runs require neither X11 nor input devices.

Raw `mouse_move(dx,dy)`, standalone `move`/`move-to`, and recorded-event replay
keep their existing units and behavior. Device deltas are not screen pixels;
the recorded `MouseTrajectory` remains a delta path, not measured cursor motion.
Prefer `mouse_move_to` for precision. Cursor feedback cannot remove remote-model
latency or replace fresh screenshots after navigation and focus changes.

The package also installs a standalone Rust frame-splicing tool. Its output
directory must not already contain files named `frame-*.png`:

```bash
video-frames --frame ./recording.mp4 --output ./video-frames
```

With API credentials configured, you can also run the agent directly:

```bash
# Watch frames produced by a running screencast process.
computeruse agent \
  --frames ./screencast-frames \
  --instructions ./gui-steps.txt

# Process a completed recording without injecting input.
computeruse agent \
  --frame ./recording.mp4 \
  --instructions ./gui-steps.txt \
  --trace ./trajectory.jsonl

# Explicitly allow model-selected mouse and keyboard input.
computeruse agent \
  --frames ./screencast-frames \
  --instructions ./gui-steps.txt \
  --execute
```

Use prerecorded video for offline evaluation. `--frame --execute` is still
accepted, with a warning: the recording cannot show the effects of actions sent
to the current desktop, even when local cursor arrival can be measured.

### Concurrent Recording And Agent

`record.sh` runs one FFmpeg process that records the X11 display at 30 FPS while
also sampling temporary observations at 10 FPS. Set `COMPUTERUSE_OBSERVATION_FPS`
to an integer from 1 to 30 to trade capture cost against feedback latency. PNG
observations use fast compression without changing resolution or image quality.
The agent watches those images through `--frames`, but requests are serialized:
Astra receives one bounded
frame trajectory per policy decision, not a continuous 10 FPS stream. The live
frame watcher polls every 20 ms, bounded by the remaining frame timeout. After
an executed input batch, already-published frames and the in-memory screenshot
history are discarded. The next decision requires an image captured at least
200 ms after input finishes, allowing focus changes to render. `record.sh` names
observations `frame-capture-<Unix microseconds>.png` using original X11 capture
timestamps, so encoder backlog cannot masquerade as fresh feedback. Other frame
producers may use this naming convention; ordinary filenames fall back to file
modification time, which cannot detect capture-to-encoding delays. Empty and wait-only batches retain images
published during the decision, avoiding an extra capture interval, but never
reuse the same observation. Publication does not guarantee the GUI has
finished rendering, so the policy is instructed to wait for delayed feedback
rather than blindly retype executed keys. When the agent completes or
fails, the script stops FFmpeg cleanly, finalizes the MP4, and removes the
temporary images. Fragmented MP4 output flushes at keyframes (at most about two
seconds apart at 30 FPS), keeping completed fragments playable after a forced
termination; the unfinished fragment can still be lost. The output path must
not already exist. JSONL transitions include the policy rationale to help
diagnose repeated actions and focus mistakes.

```bash
printf '%s\n' 'Open the browser settings page.' > gui-steps.txt

# Safe default: record and print decisions without controlling the GUI.
./record.sh ./recording.mp4 ./gui-steps.txt

# Allow the feedback loop to execute model-selected actions and save a trace.
./record.sh ./recording.mp4 ./gui-steps.txt \
  --execute \
  --trace ./trajectory.jsonl
```

Additional arguments after the instructions path are forwarded to
`computeruse agent`. Set `DISPLAY` to select a different X11 display and
`COMPUTERUSE_BIN` to override the executable used by the script. If Azure
environment variables are unset, the script reads the API key from line 1 and
the endpoint from line 3 of the ignored local `ast` file. Set
`COMPUTERUSE_CREDENTIALS` to use a different credentials file.

### Video Acceleration

Recording and offline frame extraction use FFmpeg's VA-API support when a
working Intel or AMD render node is available. This uses the GPU's video engine
for supported H.264 encoding and video decoding. X11 capture, PNG observations,
PNG output, and the remote vision model remain CPU or remote operations; this
does not run the model locally or add general-purpose GPU acceleration.

| Environment variable | Behavior |
| --- | --- |
| `COMPUTERUSE_VIDEO_ACCEL=auto` | Default. Try accessible VA-API devices, then report and use software if none works. |
| `COMPUTERUSE_VIDEO_ACCEL=vaapi` | Require VA-API and fail with the FFmpeg diagnostic if it cannot be used. |
| `COMPUTERUSE_VIDEO_ACCEL=off` | Use software without probing render nodes. |
| `COMPUTERUSE_VAAPI_DEVICE=/dev/dri/renderD128` | Try only this render node; never substitute another device. |

The FFmpeg build must include VA-API support and the appropriate Mesa or Intel
media driver must be installed. The user running `computeruse` needs read and
write permission on the selected `/dev/dri/renderD*` node, commonly through the
`render` group. The program does not install drivers, change permissions, or
require root. `vainfo --display drm --device /dev/dri/renderD128` is useful for
checking driver profiles but is not required at runtime.

Render-node numbers do not identify integrated GPUs. On a multi-GPU machine,
inspect `udevadm info --query=property --name=/dev/dri/renderD128` and the
corresponding `/sys/class/drm/renderD128/device` link, or compare devices with
`lspci -k`, then set `COMPUTERUSE_VAAPI_DEVICE` explicitly. The selected mode,
device, and any software fallback are printed to stderr.

Recording performs a short, bounded H.264 encode preflight at the actual screen
size. Automatic fallback is limited to that preflight; a capture failure after
startup stops the agent and is reported so stale observations are never used.
Offline extraction validates decoding against the input itself. In `auto`, a
failed hardware decode removes its numbered partial frames and retries once in
software while preserving unrelated files in the output directory.

### Execution Limits

Unless `--frame-output` is supplied, extracted frames
are kept in a temporary directory and removed when the command exits. The
agent samples videos at 2 FPS and processes one sampled frame per step in
offline mode. In live mode, `--frame-timeout-ms` controls how long it waits for
a new observation and defaults to 10 seconds. `--execute` is deliberately
required for input injection.
Policy actions are atomic key taps/hotkeys and mouse clicks rather than
persistent holds. Each decision is limited to 16 actions, relative movement to
32767 units per axis, scrolling to 100 units, and waits to 30 seconds.

This is an online visual policy/evaluation loop, not an in-process model
trainer. The `Policy` trait and JSONL transitions are the integration points
for a separate RL trainer or replay buffer.

## Input CLI And Library

The input layer can be used without a model or API credentials. It is a
Linux-focused refactor of [`boppreh/mouse`](https://github.com/boppreh/mouse)
and [`boppreh/keyboard`](https://github.com/boppreh/keyboard), using the kernel's
`evdev` and `uinput` interfaces for capture and injection.

```bash
# Click, scroll, and move the pointer.
computeruse click left
computeruse click right --count 2
computeruse wheel -3
computeruse move 100 -40 --duration-ms 500

# Absolute coordinates are normalized to 0..=65535 on each axis.
computeruse move-to 32768 32768

# Listen, record until Escape, and replay twice as fast.
computeruse listen
computeruse record session.json
computeruse play session.json --speed 2
computeruse play session.json --no-buttons

# Hook, send, record, and replay keyboard events.
computeruse keyboard-listen
computeruse hotkey 'ctrl+shift+a'
computeruse keyboard-record keys.json --stop-key escape
computeruse keyboard-play keys.json --speed 2
```

Before capture starts, `record` queries the pointer through X11 and moves it
to screen 0 pixel `(960, 540)`, making that position the origin for recorded
relative movement. Mouse buttons are captured through XInput2 so physical
clicks and libinput touchpad tap-to-click gestures are retained. `play` repeats
and verifies the same calibration. Both commands print the before and after
locations in `xdotool getmouselocation` format. Mouse recordings contain only
mouse events, so the Escape stop key is not included. A replay speed of `0`
removes delays.

Use the same input primitives from Rust:

```rust,no_run
use computeruse::{Button, Controller, LinuxMouse};

fn main() -> std::io::Result<()> {
    let mut mouse = Controller::new(LinuxMouse::new()?);
    mouse.click(Button::Left)?;
    mouse.move_relative(100, 20)?;
    mouse.wheel(-1)?;
    Ok(())
}
```

`Listener::new()?.listen(|event| println!("{event:?}"))` listens to global mouse
events. `Controller<B>` is generic over `MouseBackend`, allowing applications
to use a fake backend in unit tests without opening `/dev/uinput`.
`KeyboardController<B>` provides the equivalent keyboard abstraction. Key
recordings retain scan codes, normalized names, up/down state, repeat markers,
and timestamps. Hotkeys press keys in order and release them in reverse order.
The `Policy` trait lets applications integrate their own decision policy.

## Wayland And Coordinates

Wayland intentionally does not expose a global pointer-position query. This
project records movement as relative deltas. The standalone `listen`, `move`,
and `move-to` operations still work without X11, but deterministic `record` and
`play` initialization and proportional agent cursor control require an X11
session. The agent's explicit `--mouse-control direct` mode retains unverified
Wayland injection. `move-to` creates an absolute
virtual input device and uses normalized coordinates, where `(0, 0)` is the
upper-left and `(65535, 65535)` is the lower-right of the compositor's mapped
desktop.

The exact mapping across multiple monitors is compositor policy. Relative
movement is the most portable choice when exact monitor geometry is unknown.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo doc --no-deps
```

Unit tests do not require root or input devices. Hardware behavior depends on
the running kernel and compositor, and should also be smoke-tested on the
target machine.

See [`docs/MIGRATION.md`](docs/MIGRATION.md) for differences from the Python
package and [`CONTRIBUTING.md`](CONTRIBUTING.md) for contribution guidance.

## License

MIT. The original `boppreh/mouse` copyright and attribution are retained in
[`LICENSE`](LICENSE).
