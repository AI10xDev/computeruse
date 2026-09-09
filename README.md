# computeruse

`computeruse` is a Rust library and command-line tool for global mouse and
keyboard capture and control on Ubuntu. It is a Linux-focused refactor of
[`boppreh/mouse`](https://github.com/boppreh/mouse) and
[`boppreh/keyboard`](https://github.com/boppreh/keyboard) that uses the
kernel's `evdev` and `uinput` interfaces for capture and injection, plus X11
for deterministic record/play cursor calibration.

The kernel interfaces work in both Xorg and Wayland sessions and are suitable
for Ubuntu 26.04. Record/play cursor calibration specifically requires X11.

## Features

- Capture button, relative movement, vertical wheel, and horizontal wheel events.
- Inject clicks, button presses, movement, scrolling, and drag operations.
- Move to normalized absolute coordinates through a virtual absolute pointer.
- Record events as portable JSON and replay them with timing and type filters.
- Hook, record, replay, and inject keyboard keys and simultaneous hotkeys.
- Splice videos into ordered PNG frames and retain a bounded visual trajectory.
- Use an OpenAI-compatible gpt-Astra vision policy to propose or execute GUI actions.
- Use the same functionality from the CLI or the Rust library.
- Test high-level behavior without access to physical input hardware.

## Requirements

- Linux with `evdev` and `uinput` enabled (standard in Ubuntu kernels).
- An X11 session and a screen at least 961 by 541 pixels for mouse record/play calibration.
- Rust 1.88 or newer to build from source.
- FFmpeg on `PATH` for video frame extraction and visual-agent processing.
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

## CLI

```bash
# Click and scroll.
computeruse click left
computeruse click right --count 2
computeruse wheel -3

# Relative movement, immediately or over 500 ms.
computeruse move 100 -40
computeruse move 100 -40 --duration-ms 500

# Absolute movement uses normalized coordinates (0..=65535 on each axis).
computeruse move-to 32768 32768

# Print newline-delimited JSON events until interrupted.
computeruse listen

# Home the pointer to X11 pixel (960, 540), record until Escape is pressed, then home it
# again and replay twice as fast.
computeruse record session.json
computeruse play session.json --speed 2

# Replay only movement and wheel events.
computeruse play session.json --no-buttons

# Hook, send, record, and replay keyboard events.
computeruse keyboard-listen
computeruse hotkey 'ctrl+shift+a'
computeruse keyboard-record keys.json --stop-key escape
computeruse keyboard-play keys.json --speed 2
```

Before capture starts, the `record` command queries the pointer through X11 and
moves it to screen 0 pixel `(960, 540)`, making that position the origin for the
recorded relative movement. Mouse buttons are captured through XInput2 so both
physical clicks and libinput touchpad tap-to-click gestures are retained. The
`play` command repeats and verifies the same calibration before replaying events.
Both commands print the before and after locations in `xdotool getmouselocation`
format. Press Escape to stop recording.
The Escape event is not included because mouse recordings contain only mouse
events. A speed of `0` replays events without delays.

## Library

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

Listen to global events:

```rust,no_run
use computeruse::Listener;

fn main() -> std::io::Result<()> {
    Listener::new()?.listen(|event| println!("{event:?}"))
}
```

`Controller<B>` is generic over `MouseBackend`, allowing applications to use a
fake backend in unit tests without opening `/dev/uinput`.

`KeyboardController<B>` provides the equivalent keyboard abstraction. Key
recordings retain the Linux scan code, normalized key name, up/down state,
repeat marker, and timestamp. Hotkeys press keys in the listed order and
release them in reverse order, matching `boppreh/keyboard` behavior.

## Visual Agent

The `agent` command evaluates GUI instructions from a text file against either
a live frame directory supplied through `--frames` or a recorded video supplied
through `--frame`. In live mode it waits for new PNG, JPEG, or WebP images after
each decision, providing the state-estimation feedback loop needed to observe
the result of executed actions. Each policy call receives the latest bounded
trajectory and prior transitions, then emits typed mouse, keyboard, scroll, or
wait actions. The transition log includes the policy's progress estimate and
its change as a reward signal for evaluation or offline reinforcement learning.

The package also installs a standalone Rust frame-splicing tool. Its output
directory must not already contain files named `frame-*.png`:

```bash
video-frames --frame ./recording.mp4 --output ./video-frames
```

Set the Azure API key and either a Foundry project endpoint or Azure OpenAI
endpoint:

```bash
export AZURE_OPENAI_API_KEY=...
export AZURE_OPENAI_ENDPOINT=https://RESOURCE.services.ai.azure.com/api/projects/PROJECT

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
  --frame ./recording.mp4 \
  --frame-output ./extracted-frames \
  --instructions ./gui-steps.txt \
  --execute
```

### Concurrent Recording And Agent

`record.sh` runs one FFmpeg process that records the X11 display at 30 FPS while
also sampling temporary observations at 2 FPS. The agent watches those images
through `--frames`, but requests are serialized: Astra receives one bounded
frame trajectory per policy decision, not a continuous 2 FPS stream. After an
executed action, frames captured before that action completed are discarded so
the next decision observes post-action state. When the agent completes or
fails, the script stops FFmpeg cleanly, finalizes the MP4, and removes the
temporary images. The output path must not already exist.

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

The default model is `gpt-6-astra`; override it with `--model` for the name
exposed by your endpoint. Unless `--frame-output` is supplied, extracted frames
are kept in a temporary directory and removed when the command exits. The
agent processes one video frame per step in offline mode. In live mode,
`--frame-timeout-ms` controls how long it waits for a new observation and
defaults to 10 seconds. `--execute` is deliberately required for input
injection.
Policy actions are atomic key taps/hotkeys and mouse clicks rather than
persistent holds. Each decision is limited to 16 actions, relative movement to
32767 units per axis, scrolling to 100 units, and waits to 30 seconds.

This is an online visual policy/evaluation loop, not an in-process model
trainer. The `Policy` trait and JSONL transitions are the integration points
for a separate RL trainer or replay buffer.

## Wayland And Coordinates

Wayland intentionally does not expose a global pointer-position query. This
project records movement as relative deltas. The standalone `listen`, `move`,
and `move-to` operations still work without X11, but deterministic `record` and
`play` initialization requires an X11 session. `move-to` creates an absolute
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
