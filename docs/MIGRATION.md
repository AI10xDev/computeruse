# Migrating From `boppreh/mouse`

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

## Recording Format

Recordings are JSON arrays. Each event has a `type` discriminator and Unix
timestamp in fractional seconds:

```json
[
  {"type":"button","button":"left","state":"down","time":1788874200.1},
  {"type":"move","dx":12,"dy":-3,"time":1788874200.2},
  {"type":"button","button":"left","state":"up","time":1788874200.3}
]
```
