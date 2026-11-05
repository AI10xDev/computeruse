"""Isolated launcher tests: Python stand-ins never call a desktop or model API."""

import base64
import importlib.util
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock


LAUNCHER = Path(__file__).resolve().parents[1] / "scripts/computeruse-wayland.py"
SPEC = importlib.util.spec_from_file_location("wayland_launcher", LAUNCHER)
launcher = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(launcher)
REAL_POPEN = subprocess.Popen
PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8A"
    "AwMCAO+aX1sAAAAASUVORK5CYII="
)

# Real pipes, exit statuses and signals, but only synthetic files in a private
# temporary directory. These programs receive an allowlisted environment.
GRIM = r'''
import base64, os, pathlib, sys
root = pathlib.Path(os.environ["MOCK_ROOT"])
counter = root / "captures"
count = int(counter.read_text()) + 1 if counter.exists() else 1
counter.write_text(str(count))
assert sys.argv[1:5] == ["-t", "png", "-l", "1"]
mode = os.environ.get("GRIM_MODE", "ok")
if mode == "fail" or (mode == "fail-after-first" and count > 1):
    sys.stderr.buffer.write(b"mock compositor unavailable\xff\n")
    sys.exit(4)
if mode != "missing":
    with open(sys.argv[5], "wb") as image:
        if mode == "oversize":
            image.truncate(20 * 1024 * 1024 + 1)
        elif mode == "limit":
            image.truncate(20 * 1024 * 1024)
        elif mode != "empty":
            image.write(base64.b64decode(os.environ["MOCK_PNG"]))
'''

AGENT = r'''
import json, os, pathlib, signal, sys, time
root = pathlib.Path(os.environ["MOCK_ROOT"])
assert sys.argv[1] == "agent"
frames = pathlib.Path(sys.argv[sys.argv.index("--frames") + 1])
assert list(frames.glob("frame-capture-*.png"))
assert not (frames / "observation.tmp").exists()
def stopped(signum, frame):
    (root / "stopped").write_text(json.dumps({"frames_exist": frames.is_dir()}))
    sys.exit(0)
signal.signal(signal.SIGTERM, stopped)
(root / "agent.json").write_text(json.dumps({
    "args": sys.argv[1:], "frames": str(frames), "pid": os.getpid(),
    "mode": frames.stat().st_mode & 0o777,
}))
mode = os.environ.get("AGENT_MODE", "exit")
if mode == "wait":
    time.sleep(20)
    sys.exit(99)
if mode == "signal":
    signal.signal(signal.SIGTERM, signal.SIG_DFL)
    os.kill(os.getpid(), signal.SIGTERM)
sys.exit(int(os.environ.get("AGENT_EXIT", "0")))
'''

SIGTERM_RUNNER = r'''
import importlib.util, os, runpy, shutil, subprocess, sys
from unittest import mock
spec = importlib.util.spec_from_file_location("launcher_test_fixtures", sys.argv[1])
fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixtures)
real_popen = subprocess.Popen
def spawn(command, **kwargs):
    assert command[0] in ("grim", "mock-computeruse"), command
    script = fixtures.GRIM if command[0] == "grim" else fixtures.AGENT
    return real_popen([sys.executable, "-B", "-c", script, *command[1:]], **kwargs)
sys.argv = [str(fixtures.LAUNCHER), sys.argv[2]]
with mock.patch.object(os, "geteuid", return_value=1000), \
     mock.patch.object(shutil, "which", side_effect=lambda value: value), \
     mock.patch.object(subprocess, "Popen", side_effect=spawn):
    runpy.run_path(str(fixtures.LAUNCHER), run_name="__main__")
'''


class WaylandLauncherTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="wayland-launcher-tests-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.instructions = self.root / "instructions with spaces.txt"
        self.instructions.write_text("Describe the synthetic frame; do not act.\n")
        self.frames_root = self.root / "private frames"
        self.frames_root.mkdir()
        self.env = {
            "PATH": os.defpath,
            "HOME": str(self.root),
            "WAYLAND_DISPLAY": "mock-wayland",
            "XDG_RUNTIME_DIR": str(self.root),
            "COMPUTERUSE_BIN": "mock-computeruse",
            "TMPDIR": str(self.frames_root),
            "MOCK_ROOT": str(self.root),
            "MOCK_PNG": base64.b64encode(PNG).decode("ascii"),
        }
        self.enterContext(mock.patch.dict(os.environ, self.env, clear=True))
        self.enterContext(mock.patch.object(tempfile, "tempdir", str(self.frames_root)))
        self.uid = self.enterContext(mock.patch.object(launcher.os, "geteuid", return_value=1000))
        self.which = self.enterContext(mock.patch.object(
            launcher.shutil, "which", side_effect=lambda value: value,
        ))
        self.stdout = self.enterContext(mock.patch.object(sys, "stdout", new_callable=io.StringIO))
        self.stderr = self.enterContext(mock.patch.object(sys, "stderr", new_callable=io.StringIO))
        self.commands = []
        self.children = []
        self.addCleanup(self.reap_children)
        self.popen = self.enterContext(mock.patch.object(
            launcher.subprocess, "Popen", side_effect=self.spawn,
        ))

    def spawn(self, command, **kwargs):
        self.commands.append(command)
        self.assertIn(command[0], ("grim", "mock-computeruse"))
        script = GRIM if command[0] == "grim" else AGENT
        child = REAL_POPEN(
            [sys.executable, "-B", "-c", script, *command[1:]],
            env=dict(os.environ), **kwargs,
        )
        self.children.append(child)
        return child

    def reap_children(self):
        for child in self.children:
            if child.poll() is None:
                child.kill()
            child.communicate(timeout=5)

    def assert_clean(self):
        self.assertEqual(list(self.frames_root.iterdir()), [])
        self.assertTrue(all(child.poll() is not None for child in self.children))

    def assert_rejected(self, args, message):
        with self.assertRaises(SystemExit) as raised:
            launcher.main(args)
        self.assertEqual(raised.exception.code, 2)
        self.assertIn(message, self.stderr.getvalue())
        self.popen.assert_not_called()
        self.assert_clean()

    def test_check_captures_without_agent_api_or_input(self):
        self.which.side_effect = lambda name: name if name == "grim" else None
        self.assertEqual(launcher.main(["--check"]), 0)
        self.assertEqual([command[0] for command in self.commands], ["grim"])
        self.assertFalse((self.root / "agent.json").exists())
        self.which.assert_called_once_with("grim")
        self.assertIn("No API request or input was sent", self.stdout.getvalue())
        self.assertIn("not verified", self.stdout.getvalue())
        self.assert_clean()

    def test_default_command_is_direct_and_dry_run(self):
        self.assertEqual(launcher.main([str(self.instructions)]), 0)
        agent = json.loads((self.root / "agent.json").read_text())
        self.assertEqual(agent["args"], [
            "agent", "--frames", agent["frames"], "--instructions",
            str(self.instructions), "--mouse-control", "direct",
        ])
        self.assertEqual(agent["mode"], 0o700)
        self.assertNotIn("--execute", agent["args"])
        self.assertIn("even in dry-run mode", self.stderr.getvalue())
        self.assertNotIn("real desktop input enabled", self.stderr.getvalue())
        self.assert_clean()

    def test_forwarded_arguments_with_and_without_separator(self):
        forwarded = ["--execute", "--max-steps", "5", "--model=model with spaces", "--trace", "trace.jsonl"]
        for separator in ([], ["--"]):
            with self.subTest(separator=separator):
                self.assertEqual(launcher.main([str(self.instructions), *separator, *forwarded]), 0)
                agent = json.loads((self.root / "agent.json").read_text())
                self.assertEqual(agent["args"][7:], forwarded)
                self.assertIn("unverified direct cursor control", self.stderr.getvalue())
                self.assert_clean()

    def test_reserved_overrides_are_rejected_before_capture(self):
        for option in ("--frames", "--frame", "--frame-output", "--instructions", "--mouse-control"):
            for override in ([option, "elsewhere"], [option + "=elsewhere"]):
                for separator in ([], ["--"]):
                    with self.subTest(override=override, separator=separator):
                        self.assert_rejected(
                            [str(self.instructions), *separator, *override], "managed by this launcher",
                        )

    def test_invalid_check_combinations_and_missing_instructions_argument(self):
        for args in (["--check", str(self.instructions)], ["--check", "--execute"], ["--check", "--", "--execute"]):
            with self.subTest(args=args):
                self.assert_rejected(args, "--check cannot be combined")
        self.assert_rejected([], "provide an instructions file")

    def test_root_is_rejected(self):
        self.uid.return_value = 0
        self.assert_rejected(["--check"], "never root")

    def test_missing_or_empty_session_is_rejected(self):
        for variable in ("WAYLAND_DISPLAY", "XDG_RUNTIME_DIR"):
            for value in (None, ""):
                with self.subTest(variable=variable, value=value), mock.patch.dict(os.environ):
                    if value is None:
                        del os.environ[variable]
                    else:
                        os.environ[variable] = value
                    self.assert_rejected(["--check"], "run inside your Wayland desktop")

    def test_missing_grim_is_rejected(self):
        self.which.side_effect = lambda name: None if name == "grim" else name
        self.assert_rejected(["--check"], "grim is missing")

    def test_missing_agent_is_rejected(self):
        self.which.side_effect = lambda name: name if name == "grim" else None
        self.assert_rejected([str(self.instructions)], "computeruse is missing")

    def test_missing_or_directory_instructions_are_rejected(self):
        for path in (self.root / "missing.txt", self.root):
            with self.subTest(path=path):
                self.assert_rejected([str(path)], "instructions file does not exist")

    def test_initial_capture_errors_do_not_start_agent_and_clean_up(self):
        for mode, message in (
            ("fail", "grim capture failed"), ("missing", "empty, missing, or oversized"),
            ("empty", "empty, missing, or oversized"), ("oversize", "empty, missing, or oversized"),
        ):
            with self.subTest(mode=mode), mock.patch.dict(os.environ, GRIM_MODE=mode):
                self.assertEqual(launcher.main([str(self.instructions)]), 1)
                self.assertIn(message, self.stderr.getvalue())
                self.assertFalse((self.root / "agent.json").exists())
                self.assertTrue(all(command[0] == "grim" for command in self.commands))
                self.assert_clean()
        self.assertIn("mock compositor unavailable", self.stderr.getvalue())

    def test_capture_accepts_exact_size_limit(self):
        with mock.patch.dict(os.environ, GRIM_MODE="limit"):
            path = launcher.capture(self.frames_root)
        self.assertEqual(path.stat().st_size, 20 * 1024 * 1024)
        self.assertFalse((self.frames_root / "observation.tmp").exists())

    def test_capture_timeout_terminates_and_reaps_grim(self):
        process = mock.Mock()
        process.communicate.side_effect = subprocess.TimeoutExpired("grim", 5)
        process.poll.return_value = None
        self.popen.side_effect = None
        self.popen.return_value = process
        with self.assertRaisesRegex(RuntimeError, "timed out after 5 seconds"):
            launcher.capture(self.frames_root)
        process.communicate.assert_called_once_with(timeout=5)
        process.terminate.assert_called_once_with()
        process.wait.assert_called_once_with(timeout=3)
        process.kill.assert_not_called()
        self.assertEqual(list(self.frames_root.iterdir()), [])

    def test_capture_timestamp_precedes_spawn_and_publication_is_atomic(self):
        events = []
        stamp = 1_700_000_000_123_456_789
        destination = self.frames_root / f"frame-capture-{stamp // 1000:020d}.png"
        pending = self.frames_root / "observation.tmp"
        process = mock.Mock(returncode=0)
        process.poll.return_value = 0
        real_replace = Path.replace

        def spawn(command, **kwargs):
            events.append("spawn")
            self.assertEqual(command, ["grim", "-t", "png", "-l", "1", str(pending)])
            self.assertEqual(kwargs, {"stdout": subprocess.DEVNULL, "stderr": subprocess.PIPE})
            self.assertEqual(events, ["timestamp", "spawn"])
            pending.write_bytes(PNG[:8])
            self.assertFalse(destination.exists())
            return process

        def communicate(timeout):
            self.assertEqual(timeout, 5)
            self.assertFalse(destination.exists())
            pending.write_bytes(PNG)
            events.append("complete")
            return None, b""

        def replace(source, target):
            self.assertEqual(source, pending)
            self.assertEqual(target, destination)
            self.assertEqual(source.read_bytes(), PNG)
            self.assertFalse(destination.exists())
            events.append("publish")
            return real_replace(source, target)

        self.popen.side_effect = spawn
        process.communicate.side_effect = communicate
        with mock.patch.object(launcher.time, "time_ns", side_effect=lambda: events.append("timestamp") or stamp), \
             mock.patch.object(Path, "replace", autospec=True, side_effect=replace):
            self.assertEqual(launcher.capture(self.frames_root), destination)
        self.assertEqual(events, ["timestamp", "spawn", "complete", "publish"])
        self.assertEqual(destination.read_bytes(), PNG)
        self.assertFalse(pending.exists())
        process.terminate.assert_not_called()

    def test_agent_exit_status_and_signal_are_propagated(self):
        for mode, code, expected in (("exit", "0", 0), ("exit", "7", 7), ("signal", "0", 143)):
            with self.subTest(mode=mode, code=code), mock.patch.dict(os.environ, AGENT_MODE=mode, AGENT_EXIT=code):
                self.assertEqual(launcher.main([str(self.instructions)]), expected)
                self.assert_clean()

    def test_capture_failure_stops_live_agent_before_removing_frames(self):
        with mock.patch.dict(os.environ, GRIM_MODE="fail-after-first", AGENT_MODE="wait"):
            self.assertEqual(launcher.main([str(self.instructions)]), 1)
        self.assertTrue(json.loads((self.root / "stopped").read_text())["frames_exist"])
        self.assertIn("grim capture failed", self.stderr.getvalue())
        self.assert_clean()

    def test_retention_keeps_latest_twenty_and_cleans_up(self):
        published = []
        observed_counts = []
        agent = mock.Mock()
        agent.poll.return_value = 0
        self.popen.side_effect = None
        self.popen.return_value = agent

        def capture(directory):
            path = directory / f"frame-capture-{len(published):020d}.png"
            path.write_bytes(PNG)
            published.append(path)
            return path

        def wait(timeout):
            self.assertEqual(timeout, 0.5)
            existing = sorted(published[0].parent.glob("*.png"))
            self.assertEqual(existing, published[-20:])
            observed_counts.append(len(existing))
            if len(published) < 25:
                raise subprocess.TimeoutExpired("mock-computeruse", timeout)
            return 0

        agent.wait.side_effect = wait
        with mock.patch.object(launcher, "capture", side_effect=capture):
            self.assertEqual(launcher.main([str(self.instructions)]), 0)
        self.assertEqual(len(published), 25)
        self.assertEqual(max(observed_counts), 20)
        self.assertTrue(all(not path.exists() for path in published))
        self.assert_clean()

    def test_spawn_oserror_is_reported_and_cleans_up(self):
        self.popen.side_effect = OSError("mock executable disappeared")
        self.assertEqual(launcher.main(["--check"]), 1)
        self.assertIn("mock executable disappeared", self.stderr.getvalue())
        self.assert_clean()

    def test_stop_escalates_to_kill_and_reaps(self):
        process = mock.Mock()
        process.poll.return_value = None
        process.wait.side_effect = [subprocess.TimeoutExpired("mock", 3), 0]
        launcher.stop(process)
        self.assertEqual(process.mock_calls, [
            mock.call.poll(), mock.call.terminate(), mock.call.wait(timeout=3),
            mock.call.kill(), mock.call.wait(),
        ])

    def test_stop_does_not_signal_exited_or_absent_process(self):
        launcher.stop(None)
        process = mock.Mock()
        process.poll.return_value = 0
        launcher.stop(process)
        process.terminate.assert_not_called()
        process.kill.assert_not_called()

    @unittest.skipUnless(os.name == "posix", "requires POSIX SIGTERM")
    def test_sigterm_exits_143_stops_agent_and_cleans_up(self):
        child = REAL_POPEN(
            [sys.executable, "-B", "-c", SIGTERM_RUNNER, str(Path(__file__).resolve()), str(self.instructions)],
            env=self.env | {"AGENT_MODE": "wait"},
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True,
        )
        try:
            deadline = time.monotonic() + 5
            ready = self.root / "agent.json"
            while not ready.exists() and child.poll() is None and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertTrue(ready.exists(), "mock agent did not become ready")
            child.send_signal(signal.SIGTERM)
            stdout, stderr = child.communicate(timeout=8)
            self.assertEqual(child.returncode, 143, stdout + stderr)
            self.assertTrue(json.loads((self.root / "stopped").read_text())["frames_exist"])
            self.assertEqual(list(self.frames_root.iterdir()), [])
        finally:
            # Kill only this test's process group if an assertion or timeout fails.
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.communicate(timeout=5)


if __name__ == "__main__":
    unittest.main()
