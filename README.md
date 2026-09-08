# computeruse

`computeruse` is a Rust library and command-line tool for global mouse capture
and control on Ubuntu. It is a Linux-focused refactor of
[`boppreh/mouse`](https://github.com/boppreh/mouse) that uses the kernel's
`evdev` and `uinput` interfaces instead of Python and X11.

The kernel interfaces work in both Xorg and Wayland sessions and are suitable
for Ubuntu 26.04. No display-server-specific automation API is required.

## Features

- Capture button, relative movement, vertical wheel, and horizontal wheel events.
- Inject clicks, button presses, movement, scrolling, and drag operations.
- Move to normalized absolute coordinates through a virtual absolute pointer.
- Record events as portable JSON and replay them with timing and type filters.
- Use the same functionality from the CLI or the Rust library.
- Test high-level behavior without access to physical input hardware.

## Requirements

- Linux with `evdev` and `uinput` enabled (standard in Ubuntu kernels).
- Rust 1.88 or newer to build from source.
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

# Record through the first right-button press, then replay twice as fast.
computeruse record session.json
computeruse play session.json --speed 2

# Replay only movement and wheel events.
computeruse play session.json --no-buttons
```

The `record` command includes the stop event in the file, matching the behavior
of the original Python package. Use `--stop-button middle` to select another
button. A speed of `0` replays events without delays.

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

## Wayland And Coordinates

Wayland intentionally does not expose a global pointer-position query. This
project therefore records movement as relative deltas and does not provide the
upstream `get_position()` API. `move-to` creates an absolute virtual input
device and uses normalized coordinates, where `(0, 0)` is the upper-left and
`(65535, 65535)` is the lower-right of the compositor's mapped desktop.

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
