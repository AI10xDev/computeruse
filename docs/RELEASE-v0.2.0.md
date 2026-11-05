# Computeruse v0.2.0: Arch Linux Wayland Integration

This release adds a source installer for Arch Linux and a separate live-frame
launcher for grim-compatible Wayland compositors, reusing the Rust agent and
uinput backend.

- `install-arch-wayland.sh`: pinned source build, user-local installation, full
  pacman upgrade, and optional narrowly scoped `/dev/uinput` permissions.
- `computeruse-wayland.py`: live screenshot capture, atomic timestamped frames,
  bounded retention, failure cleanup, and explicit direct-mode agent integration.
- `ARCH-WAYLAND.md`: setup, API configuration, input security, validation,
  troubleshooting, limitations, and uninstall instructions.
- Automated installer and launcher tests, plus an Arch Linux container CI job.

Download the installer and `SHA256SUMS`, verify and review the script, then run:

```bash
bash install-arch-wayland.sh
export PATH="$HOME/.local/bin:$PATH"
computeruse-wayland --check
```

Read **ARCH-WAYLAND.md** before enabling input or connecting a model endpoint.
Run as your regular desktop user. The installer does not enable input by default;
use `--enable-input` only after accepting desktop-wide injection privileges.

## Scope

This is not universal Wayland support: the launcher requires grim-compatible
capture and does not include GNOME/KDE portal integration, verified Wayland
cursor positioning, multi-monitor coordinate calibration, or MP4 recording.
Existing X11 workflows remain available. Screenshots are sent to your configured
remote model even in dry-run mode; execution requires explicit `--execute`.

Automated tests do not replace a real Arch/Wayland desktop smoke test. Live
compositor capture, uinput delivery, and model calls were not verified in the
headless release environment. No precompiled executable or model weights are
included; the installer builds the tagged Rust source with its lockfile.

## Local Verification

- Rust tests: 83 passed, 2 environment-dependent tests ignored.
- Installer/launcher tests: 39 passed, 1 real-root test skipped as a regular user;
  all 40 passed in a separate root test run using mocked system operations.
- Bash syntax, Rust formatting, Clippy with warnings denied, documentation, and
  the optimized release build passed.
- Independent integration review found no actionable issues.
- The local Docker daemon was unavailable, so the Arch container job must run
  on GitHub Actions. No real pacman upgrade or input-permission change was made
  on the release host.
