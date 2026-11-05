#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: install-arch-wayland.sh [--prefix /absolute/path] [--no-packages] [--enable-input]

Install computeruse v0.2.0 for Arch Linux and Arch-based distributions.
Run as your regular desktop user, never with sudo or as root.

  --prefix PATH    Install under PATH (default: $HOME/.local); must be absolute
  --no-packages    Skip pacman; check already-installed prerequisites instead
  --enable-input   Grant this user desktop-wide input injection via /dev/uinput
                  using the dedicated computeruse group (requires sudo/relogin)
  --help          Show this help

By default, sudo pacman -Syu --needed performs a full system upgrade and installs
base-devel, git, python, grim and ffmpeg. Rust is added
only if cargo or rustc is missing. Existing Rust toolchains must be >= 1.88.
Input permissions are unchanged unless --enable-input is explicitly supplied.
The launcher needs a running Wayland session and a compositor supported by grim.
EOF
}

die() {
    printf 'Error: %s\n' "$*" >&2
    exit 1
}

prefix=${HOME:?HOME must be set}/.local
packages=true
enable_input=false
help=false
while (($#)); do
    case "$1" in
        --help) help=true; shift ;;
        --prefix)
            (($# >= 2)) || die '--prefix requires an absolute path.'
            [[ $2 == /* ]] || die '--prefix requires an absolute path.'
            prefix=$2
            shift 2
            ;;
        --no-packages) packages=false; shift ;;
        --enable-input) enable_input=true; shift ;;
        *) die "Unknown argument: $1. Use --help for usage." ;;
    esac
done
if $help; then
    usage
    exit 0
fi
[[ $prefix == /* ]] || die 'The install prefix must be absolute; set HOME or use --prefix.'
[[ $(id -u) != 0 ]] || die 'Run this installer as a regular user, not root or sudo. Never run the agent as root.'
[[ -r /etc/os-release ]] || die 'Cannot detect Arch Linux: /etc/os-release is not readable.'
ID=
ID_LIKE=
. /etc/os-release
arch=false
for distro in ${ID:-} ${ID_LIKE:-}; do
    [[ $distro != arch ]] || arch=true
done
$arch || die 'This installer supports Arch Linux (ID=arch or arch in ID_LIKE) only.'

trap 'printf "Error: installation failed at line %s. Check the command output, dependencies, network and prefix permissions, then rerun.\n" "$LINENO" >&2' ERR
if $packages; then
    command -v sudo >/dev/null || die 'Install sudo and configure access, or use --no-packages with dependencies already installed.'
    command -v pacman >/dev/null || die 'pacman is required; use this installer on an Arch-based system.'
    package_list=(base-devel git python grim ffmpeg)
    if ! command -v cargo >/dev/null || ! command -v rustc >/dev/null; then
        package_list+=(rust)
    fi
    sudo pacman -Syu --needed "${package_list[@]}" || die 'Full system upgrade/package installation failed. Resolve pacman errors and rerun; do not use a partial upgrade.'
fi

for dependency in git python grim ffmpeg cargo rustc cc make install mktemp rm; do
    command -v "$dependency" >/dev/null || die "Missing prerequisite: $dependency. Install base-devel git python grim ffmpeg and Rust >= 1.88, or rerun without --no-packages."
done
for tool in cargo rustc; do
    version=$($tool --version) || die "$tool is unavailable. Configure a working Rust >= 1.88 toolchain (for rustup, use rustup update stable) and rerun."
    if [[ $version =~ ^$tool\ ([0-9]+)\.([0-9]+)\. ]]; then
        ((BASH_REMATCH[1] > 1 || (BASH_REMATCH[1] == 1 && BASH_REMATCH[2] >= 88))) || die "$tool >= 1.88 is required (found $version). Update your Rust toolchain; for rustup use rustup update stable."
    else
        die "Cannot determine $tool version from '$version'; configure Rust >= 1.88 and rerun."
    fi
done
python -c 'import sys; sys.exit(0 if sys.version_info.major == 3 else 1)' || die 'Python 3 is required; update the Arch python package.'
if $enable_input; then
    for dependency in sudo groupadd usermod modprobe udevadm; do
        command -v "$dependency" >/dev/null || die "Missing prerequisite for --enable-input: $dependency. Install sudo, shadow, kmod and systemd, then rerun."
    done
    printf '%s\n' 'Warning: --enable-input grants desktop-wide keyboard/mouse injection, not just access to this application.' >&2
fi

workdir=$(mktemp -d "${TMPDIR:-/tmp}/computeruse-install.XXXXXXXX") || die 'Cannot create a private temporary build directory; check TMPDIR and free space.'
trap 'rm -rf -- "$workdir"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
checkout=$workdir/checkout
git clone --depth 1 --branch v0.2.0 --single-branch -- https://github.com/AI10xDev/computeruse.git "$checkout" || die 'Cannot fetch release tag v0.2.0. Check network access and that the release tag exists on AI10xDev/computeruse.'
[[ -f $checkout/scripts/computeruse-wayland.py && -f $checkout/docs/ARCH-WAYLAND.md ]] || die 'Release v0.2.0 is missing its Wayland launcher or setup guide. Check the published release.'
cargo install --locked --path "$checkout" --root "$prefix" || die 'Cargo build/install failed. Check Rust >= 1.88, base-devel, network access, disk space and prefix write permissions; see the Cargo error above.'
install -Dm755 -- "$checkout/scripts/computeruse-wayland.py" "$prefix/bin/computeruse-wayland"
install -Dm644 -- "$checkout/docs/ARCH-WAYLAND.md" "$prefix/share/doc/computeruse/ARCH-WAYLAND.md"

if $enable_input; then
    username=$(id -un) || die 'Cannot determine the current user for input group membership.'
    printf '%s\n' 'SUBSYSTEM=="misc", KERNEL=="uinput", GROUP="computeruse", MODE="0660", OPTIONS+="static_node=uinput"' > "$workdir/70-computeruse-uinput.rules"
    printf '%s\n' uinput > "$workdir/computeruse-uinput.conf"
    sudo groupadd -f computeruse
    sudo install -D -o root -g root -m 0644 -- "$workdir/70-computeruse-uinput.rules" /etc/udev/rules.d/70-computeruse-uinput.rules
    sudo install -D -o root -g root -m 0644 -- "$workdir/computeruse-uinput.conf" /etc/modules-load.d/computeruse-uinput.conf
    sudo usermod -aG computeruse "$username"
    sudo modprobe uinput
    sudo udevadm control --reload-rules
    sudo udevadm trigger --action=add --subsystem-match=misc --sysname-match=uinput
    printf '%s\n' 'Log out completely and log back in for group membership to take effect. Never run the agent as root.'
else
    printf '%s\n' 'Input permissions were not changed. Use --enable-input only if you accept desktop-wide input injection privileges.'
fi
printf 'Installed computeruse v0.2.0 under %s\n' "$prefix"
printf 'Add %s/bin to PATH if needed. Setup guide: %s/share/doc/computeruse/ARCH-WAYLAND.md\n' "$prefix" "$prefix"
