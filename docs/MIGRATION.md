# Migrating From `boppreh/mouse` And `boppreh/keyboard`

This project preserves the intent and major operations of the Python package,
but it is not a drop-in Python API replacement.

| Python operation | Rust library | CLI |
| --- | --- | --- |
| `press(button)` | `Controller::press` | `press BUTTON` |
| `release(button)` | `Controller::release` | `release BUTTON` |
| `click(button)` | `Controller::click` | `click BUTTON` |
| `double_click(button)` | `Controller::double_click` | `click BUTTON --count 2` |
| `move(x, y, absolute=False)` | `Controller::move_relative` | `move DX DY` |
| `move(x, y, absolute=True)` | `Controller::move_absolute` | `move-to X Y` |
| `wheel(delta)` | `Controller::wheel` | `wheel DELTA` |
| `hook(callback)` | `Listener::listen` | `listen` |
| `record()` | `Listener::listen_until` | `record FILE` |
| `play(events)` | `Controller::play` | `play FILE` |

Keyboard equivalents use `KeyboardController`, `KeyboardListener`, and the
`keyboard-*` CLI commands:

| Python operation | Rust library | CLI |
| --- | --- | --- |
| `press(key)` | `KeyboardController::press` | `key-press KEY` |
| `release(key)` | `KeyboardController::release` | `key-release KEY` |
| `send(hotkey)` | `KeyboardController::send_hotkey` | `hotkey KEYS` |
| `hook(callback)` | `KeyboardListener::listen` | `keyboard-listen` |
| `record()` | `KeyboardListener::listen_until` | `keyboard-record FILE` |
| `play(events)` | `KeyboardController::play` | `keyboard-play FILE` |

## Intentional Changes

- Linux is the only supported platform. Windows and macOS modules were not
  carried into this Ubuntu-focused rewrite.
- Events use strongly typed Rust enums and serde-compatible JSON.
- Movement capture stores relative deltas. The old Linux implementation mixed
  raw `evdev` capture with X11 pointer queries, which did not work reliably in
  Wayland sessions.
- Absolute movement uses normalized `u16` coordinates rather than desktop
  pixels because Wayland does not expose global desktop geometry.
- The caller owns listener threads and cancellation. The library does not
  create hidden global threads or global mutable callback lists.
- Double clicks are emitted as two normal clicks. Linux input devices do not
  provide a distinct double-click event; desktop toolkits infer one from time
  and distance.
- Permissions are reported as I/O errors rather than requiring the process to
  run as root. ACLs or a narrowly scoped group are recommended.
- Keyboard key names cover common US-layout keys and modifiers. Any supported
  Linux key can be addressed losslessly as `code:<scan-code>`. Layout-aware
  Unicode text composition, suppression, abbreviations, and hidden global
  callback threads from the Python package are intentionally not reproduced.

## Mouse Recording Format

Recordings are JSON arrays. Each event has a `type` discriminator and Unix
timestamp in fractional seconds:

```json
[
  {"type":"button","button":"left","state":"down","time":1788874200.1},
  {"type":"move","dx":12,"dy":-3,"time":1788874200.2},
  {"type":"button","button":"left","state":"up","time":1788874200.3}
]
```

Keyboard recordings are also JSON arrays, but use a keyboard-specific schema:

```json
[
  {
    "key":{"name":"a","scan_code":30},
    "state":"down",
    "repeat":false,
    "time":1788874200.1
  },
  {
    "key":{"name":"a","scan_code":30},
    "state":"up",
    "repeat":false,
    "time":1788874200.2
  }
]
```

`keyboard-record` stops after the stop key is released, so its recording
contains a balanced down/up pair.
