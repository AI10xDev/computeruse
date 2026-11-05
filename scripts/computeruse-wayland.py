#!/usr/bin/env python3
"""Publish grim observations to computeruse's existing live-frame interface."""

import argparse
from collections import deque
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time


def stop(process):
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def capture(directory):
    # Timestamp the request, not publication: slow encoding must not bypass the
    # agent's post-action freshness barrier. This is not a presentation timestamp.
    stamp = time.time_ns() // 1000
    pending = directory / "observation.tmp"
    destination = directory / f"frame-capture-{stamp:020d}.png"
    process = subprocess.Popen(
        ["grim", "-t", "png", "-l", "1", str(pending)],
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
    )
    try:
        _, errors = process.communicate(timeout=5)
        if process.returncode != 0:
            raise RuntimeError(
                "grim capture failed; a grim-compatible Wayland compositor is required "
                "(this launcher does not support GNOME/Mutter or KDE/KWin).\n"
                + errors.decode(errors="replace").strip()
            )
    except subprocess.TimeoutExpired as error:
        raise RuntimeError("grim capture timed out after 5 seconds") from error
    finally:
        stop(process)
    if not pending.is_file() or not 0 < pending.stat().st_size <= 20 * 1024 * 1024:
        raise RuntimeError("grim produced an empty, missing, or oversized image (limit: 20 MiB)")
    pending.replace(destination)
    return destination


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Live Wayland observations via grim; dry-run unless --execute is passed.",
        epilog="Example: computeruse-wayland instructions.txt -- --execute --max-steps 5",
    )
    parser.add_argument("instructions", nargs="?", type=Path)
    parser.add_argument("--check", action="store_true", help="test capture without API calls or input")
    args, agent_args = parser.parse_known_args(argv)
    if agent_args[:1] == ["--"]:
        agent_args = agent_args[1:]
    if args.check and (args.instructions or agent_args):
        parser.error("--check cannot be combined with instructions or agent options")
    if not args.check and not args.instructions:
        parser.error("provide an instructions file, or use --check")
    reserved = {"--frames", "--frame", "--frame-output", "--instructions", "--mouse-control"}
    if any(value.split("=", 1)[0] in reserved for value in agent_args):
        parser.error("frame sources, instructions and mouse control are managed by this launcher")
    if os.geteuid() == 0:
        parser.error("run as your regular Wayland desktop user, never root")
    if not os.environ.get("WAYLAND_DISPLAY") or not os.environ.get("XDG_RUNTIME_DIR"):
        parser.error("run inside your Wayland desktop with WAYLAND_DISPLAY and XDG_RUNTIME_DIR set")
    if shutil.which("grim") is None:
        parser.error("grim is missing; install the Arch grim package")
    if args.instructions and not args.instructions.is_file():
        parser.error(f"instructions file does not exist: {args.instructions}")
    sibling = Path(__file__).resolve().with_name("computeruse")
    binary = os.environ.get("COMPUTERUSE_BIN", str(sibling) if sibling.is_file() else "computeruse")
    if not args.check and shutil.which(binary) is None:
        parser.error("computeruse is missing; install it or set COMPUTERUSE_BIN")
    if "--execute" in agent_args:
        print("Warning: real desktop input enabled with unverified direct cursor control. "
              "Use a single unscaled output and supervise all actions.", file=sys.stderr)
    agent = None
    try:
        with tempfile.TemporaryDirectory(prefix="computeruse-wayland-") as temporary:
            directory = Path(temporary)
            frames = deque([capture(directory)])
            if args.check:
                print("Wayland capture succeeded. No API request or input was sent. "
                      "Cursor mapping and input permissions are not verified.")
                return 0
            print("Screenshots will be sent to your configured model endpoint, even in dry-run mode.",
                  file=sys.stderr)
            agent = subprocess.Popen([
                binary, "agent", "--frames", str(directory),
                "--instructions", str(args.instructions), "--mouse-control", "direct", *agent_args,
            ])
            try:
                while True:
                    try:
                        returncode = agent.wait(timeout=0.5)
                        return returncode if returncode >= 0 else 128 - returncode
                    except subprocess.TimeoutExpired:
                        frames.append(capture(directory))
                        while len(frames) > 20:
                            frames.popleft().unlink(missing_ok=True)
            finally:
                # Stop the reader before removing its private frame directory.
                stop(agent)
    except (OSError, RuntimeError) as error:
        print(f"Error: {error}", file=sys.stderr)
        return 1


def interrupted(signum, _frame):
    raise KeyboardInterrupt(signum)


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, interrupted)
    try:
        sys.exit(main())
    except KeyboardInterrupt as error:
        sys.exit(128 + (error.args[0] if error.args else signal.SIGINT))
