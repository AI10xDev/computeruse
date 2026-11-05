# Arch Linux Wayland Setup

Version **0.2.0** adds an Arch installer and the `computeruse-wayland` live
observation launcher. The Rust input backend and agent are reused unchanged.

## Support And Limits

| Component | Status |
| --- | --- |
| Arch Linux / Arch-based distributions | Source installation with pacman dependencies and Rust >= 1.88 |
| Sway and other grim-compatible Wayland compositors | Live screenshots through `grim`; run `--check` to verify your session |
| Hyprland | Requires a version exposing capture protocols supported by your installed grim; verify with `--check` |
| GNOME/Mutter, KDE/KWin Wayland | Not supported by this launcher; no ScreenCast/RemoteDesktop portal backend is included |
| Mouse and keyboard injection | Existing kernel uinput backend, explicit `--execute`, unverified direct cursor control |
| Multiple monitors, scaling, rotation | Absolute screenshot-to-pointer mapping is not verified; start with one unscaled output |
| X11 recording and verified cursor control | Existing `record.sh` and proportional mode remain X11-only |
| Wayland MP4 recording | Not included; this launcher produces temporary agent observations, not a video |

Do not set `XDG_SESSION_TYPE=x11` to bypass Wayland restrictions. XWayland is
not a substitute for capturing and controlling the native Wayland desktop.

## Install

Run as your **regular desktop user**, not with `sudo`. The installer prompts for
sudo only for package installation and optional input permissions. It performs
a **full Arch system upgrade** (`pacman -Syu --needed`), not a partial upgrade.
Review the script and normal Arch upgrade notices first. Back up important data.

Download the installer and checksums from the same versioned release:

```bash
curl -fLO https://github.com/AI10xDev/computeruse/releases/download/v0.2.0/install-arch-wayland.sh
curl -fLO https://github.com/AI10xDev/computeruse/releases/download/v0.2.0/SHA256SUMS
# This manifest also lists other assets; check the installer entry.
sha256sum --check --ignore-missing SHA256SUMS
less install-arch-wayland.sh
bash install-arch-wayland.sh
export PATH="$HOME/.local/bin:$PATH"
computeruse --version
```

The checksum detects a corrupted download; it is not an independent signature.
The script fetches the fixed `v0.2.0` Git tag from `AI10xDev/computeruse` over
HTTPS and runs `cargo install --locked` as your user. Installation requires
network access to GitHub and the Cargo registry, disk space, and build time.
No API keys are requested, read from the checkout, or installed.

Packages: `base-devel`, `git`, `python`, `grim`, `ffmpeg`, and `rust` if a
Rust toolchain is absent. Existing Rust/Cargo must be at least 1.88. A rustup
installation is retained rather than replaced with the conflicting Arch rust
package. FFmpeg supports the existing video tools; the Wayland launcher uses
grim, Python's standard library, and the existing Rust agent.

Options:

```bash
# Use already-installed dependencies; no pacman operation.
bash install-arch-wayland.sh --no-packages

# Install to a different user-writable absolute prefix.
bash install-arch-wayland.sh --prefix "$HOME/apps/computeruse"

# Explicitly enable desktop-wide input injection (see security note below).
bash install-arch-wayland.sh --enable-input
```

Installed files are `PREFIX/bin/computeruse`, `PREFIX/bin/video-frames`,
`PREFIX/bin/computeruse-wayland`, and
`PREFIX/share/doc/computeruse/ARCH-WAYLAND.md`. The default prefix is
`$HOME/.local`. Add its `bin` directory to your shell's PATH permanently if
needed; the launcher also finds the Rust executable next to itself.

## Input Permissions

Screenshot checks and agent dry runs need no input permissions. `--enable-input`
creates a dedicated `computeruse` group, adds your current user, and installs:

- `/etc/udev/rules.d/70-computeruse-uinput.rules`: only `/dev/uinput`, group `computeruse`, mode `0660`.
- `/etc/modules-load.d/computeruse-uinput.conf`: load `uinput` on boot.

It loads the module and reloads/triggers udev rules. **Log out fully and back
in** before trying execution; existing sessions do not acquire new groups.

```bash
id -nG
ls -l /dev/uinput
test -w /dev/uinput && printf '%s\n' 'uinput is writable'
```

This grants keyboard and mouse injection across the desktop, not just this
application. Every process running as your user gains that capability. Grant it
only to trusted accounts. No world-writable devices, broad `input` group access,
or read access to `/dev/input/event*` are configured. Raw device listening and
keyboard event recording require separate permissions and are not configured
by this installer. Other local udev rules can override these permissions;
inspect them if the device ownership differs. Never run the agent as root.

## Capture Check

Inside a running supported Wayland desktop terminal:

```bash
computeruse-wayland --check
```

This takes and discards one screenshot with no API request or input injection.
`WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` must belong to your logged-in session.
Do not launch through sudo or invent these variables in an unrelated SSH session.
Capture failure, missing protocols, or a five-second capture timeout is reported
as an error; the launcher does not silently fall back to X11.

## Run The Agent

Configure your image-capable model endpoint using the environment, as described
in the main README. For example, with a compatible Chat Completions endpoint:

```bash
read -rs -p 'API key: ' OPENAI_API_KEY; printf '\n'
export OPENAI_API_KEY
# Optional: export OPENAI_BASE_URL=https://YOUR-ENDPOINT/v1
printf '%s\n' 'Describe the visible desktop without making changes.' > instructions.txt
computeruse-wayland instructions.txt -- --max-steps 3
```

The default model is `gpt-6-astra`; pass `--model YOUR_VISION_MODEL` when your
endpoint uses another model/deployment name. Azure endpoints instead use
`AZURE_OPENAI_API_KEY` and `AZURE_OPENAI_ENDPOINT`, with the Responses API.
Do not put keys in source files or shell history. No local model is bundled.

**Even a dry run sends screenshots to the configured remote endpoint.** Use a
non-sensitive test desktop. After inspecting dry-run decisions and setting up
permissions, explicitly opt into real input:

```bash
computeruse-wayland instructions.txt -- --execute --max-steps 3 --trace trajectory.jsonl
```

Extra options after `--` go to `computeruse agent`. The launcher owns the frame
source, instruction file, and `--mouse-control direct`; overriding these is
rejected. `COMPUTERUSE_BIN` can select another trusted Rust executable.
There is **no verified Wayland pointer-arrival feedback**. Supervise execution,
start with a single output at scale 1, and stop with Ctrl+C if mapping is wrong.
Do not treat a successful screenshot check as proof that clicking works.

The launcher captures approximately twice per second plus capture time. Each
complete PNG is atomically published with a pre-capture Unix-microsecond
timestamp, preserving the agent's post-action freshness checks even if capture
is slow. This is a conservative request timestamp, not an exact compositor
presentation timestamp. It retains at most 20 observations in a private
temporary directory, stops the agent on capture failure, propagates agent exit
status, and removes screenshots on normal exit or handled signals. SIGKILL or
machine failure can leave temporary data; treat traces and screenshots as
sensitive. MP4s are not produced.

## Validation And Review

The integration review identified three blockers in the original live workflow:
`record.sh` uses `xdpyinfo`/FFmpeg `x11grab`; the agent defaults to proportional
X11 cursor feedback; mouse `record`/`play` initialization also uses X11.
The new launcher avoids the first two via grim and explicit direct mode. It
does not change mouse record/play or pretend their calibration works on Wayland.

The existing `agent --frames` interface (`src/agent.rs`) already validates image
freshness and clears stale observations after input. Existing `src/linux.rs`
provides uinput injection. No new privileged daemon, portal grant, background
service, or Rust backend is needed for this scoped integration.

Automated validation commands:

```bash
bash -n scripts/install-arch-wayland.sh
python3 -B -m unittest discover -s tests -p 'test_*.py' -v
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
```

Python tests mock system installation and capture/input dependencies. CI also
builds/tests in an Arch container without a compositor. These checks do **not**
prove live Wayland capture, input delivery, compositor mapping, or model-endpoint
compatibility. Before relying on the integration, verify `--check`, a supervised
dry run, and consented pointer/keyboard actions on the actual target desktop.

## Uninstall

For the default prefix:

```bash
cargo uninstall --root "$HOME/.local" computeruse
rm -- "$HOME/.local/bin/computeruse-wayland"
rm -- "$HOME/.local/share/doc/computeruse/ARCH-WAYLAND.md"
```

If input setup was enabled, remove your membership and the installer-owned
configuration only when no other installation uses it:

```bash
sudo gpasswd -d "$(id -un)" computeruse
sudo rm -- /etc/udev/rules.d/70-computeruse-uinput.rules
sudo rm -- /etc/modules-load.d/computeruse-uinput.conf
sudo udevadm control --reload-rules
```

Log out and reboot to clear existing memberships and device permissions. Delete
the `computeruse` group only if it has no remaining users or consumers. Packages
are not removed automatically because other applications may need them.
