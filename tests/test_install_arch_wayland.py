"""Installer integration tests with shell mocks; no network or privileged writes."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


INSTALLER = Path(__file__).resolve().parents[1] / "scripts/install-arch-wayland.sh"

# BASH_ENV intercepts only platform and external operations. The installer itself
# keeps its real /etc/os-release path and has no test-specific switches.
MOCKS = r'''
log() {
    printf '%s\t' "$@" >> "$MOCK_LOG"
    printf '\n' >> "$MOCK_LOG"
}
id() {
    case "$1" in
        -u) printf '%s\n' "${MOCK_UID:-1000}" ;;
        -un) printf '%s\n' desktop-user ;;
        *) return 99 ;;
    esac
}
function .() {
    [[ $1 == /etc/os-release ]] || return 99
    ID=${MOCK_ID:-arch}
    ID_LIKE=${MOCK_ID_LIKE:-}
}
command() {
    if [[ ${1:-} == -v ]]; then
        [[ " ${MOCK_MISSING:-} " != *" $2 "* ]] || return 1
        printf '%s\n' "$2"
    else
        builtin command "$@"
    fi
}
sudo() {
    log sudo "$@"
    [[ ${MOCK_FAIL:-} != "$1" ]] || return 1
    case "$1" in
        pacman) unset MOCK_MISSING ;;
        install)
            local source=${@: -2:1} destination=${@: -1}
            builtin command cp -- "$source" "$MOCK_SYSTEM/${destination##*/}"
            ;;
        groupadd|usermod|modprobe|udevadm) ;;
        *) return 99 ;;
    esac
}
git() {
    log git "$@"
    [[ ${MOCK_FAIL:-} != git ]] || return 1
    local checkout=${@: -1}
    builtin command mkdir -p "$checkout/scripts" "$checkout/docs"
    printf '#!/usr/bin/env python\n' > "$checkout/scripts/computeruse-wayland.py"
    printf 'Wayland setup\n' > "$checkout/docs/ARCH-WAYLAND.md"
    if [[ ${MOCK_RELEASE_MISSING:-} == 1 ]]; then
        builtin command rm "$checkout/docs/ARCH-WAYLAND.md"
    fi
    # A checkout can contain sensitive files; none may be copied to the prefix.
    printf 'DO NOT COPY\n' > "$checkout/astra"
    printf 'DO NOT COPY\n' > "$checkout/ast"
    log temporary-mode "$(builtin command stat -c %a "${checkout%/*}")"
}
cargo() {
    log cargo "$@"
    [[ ${MOCK_FAIL:-} != cargo ]] || return 1
    if [[ $1 == --version ]]; then
        printf 'cargo %s (mock)\n' "${MOCK_VERSION:-1.88.0}"
    elif [[ ${MOCK_FAIL:-} == build ]]; then
        return 1
    fi
}
rustc() { printf 'rustc %s (mock)\n' "${MOCK_VERSION:-1.88.0}"; }
python() { log python "$@"; [[ ${MOCK_FAIL:-} != python ]]; }
install() { log install "$@"; builtin command install "$@"; }
'''


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="installer-tests-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.home = self.root / "home"
        self.home.mkdir()
        self.builds = self.root / "builds"
        self.builds.mkdir()
        self.system = self.root / "mock-system"
        self.system.mkdir()
        self.log = self.root / "commands"
        self.env_file = self.root / "mock.bash"
        self.env_file.write_text(MOCKS)
        self.env = {
            "PATH": "/usr/bin:/bin",
            "HOME": str(self.home),
            "TMPDIR": str(self.builds),
            "BASH_ENV": str(self.env_file),
            "MOCK_LOG": str(self.log),
            "MOCK_SYSTEM": str(self.system),
        }

    def run_installer(self, *args, **env):
        return subprocess.run(
            ["bash", str(INSTALLER), *args],
            env=self.env | env,
            text=True,
            capture_output=True,
            timeout=15,
        )

    def commands(self):
        if not self.log.exists():
            return []
        return [line.rstrip("\t").split("\t") for line in self.log.read_text().splitlines()]

    def assert_success(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(list(self.builds.iterdir()), [], "build checkout was not cleaned up")

    def test_help_has_no_side_effects_even_as_root(self):
        result = self.run_installer("--help", MOCK_UID="0")
        self.assert_success(result)
        self.assertIn("v0.2.0", result.stdout)
        self.assertIn("--enable-input", result.stdout)
        self.assertEqual(self.commands(), [])

    def test_invalid_arguments_have_no_side_effects(self):
        for args in [
            ("--wat",), ("--prefix",), ("--prefix", "relative"),
            ("--prefix", ""), ("--prefix", "--enable-input"),
            ("--no-packages=yes",), ("--help", "--wat"), ("positional",),
        ]:
            with self.subTest(args=args):
                result = self.run_installer(*args)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Error:", result.stderr)
                self.assertEqual(self.commands(), [])

    def test_root_refusal(self):
        result = self.run_installer(MOCK_UID="0")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("regular user", result.stderr)
        self.assertEqual(self.commands(), [])

    @unittest.skipUnless(os.geteuid() == 0, "real-root check requires a root test runner")
    def test_real_root_refusal(self):
        env = self.env.copy()
        del env["BASH_ENV"]
        result = subprocess.run(
            ["bash", str(INSTALLER)], env=env, text=True, capture_output=True, timeout=15
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("regular user", result.stderr)

    def test_rejects_non_arch_and_substring_matches(self):
        for distro in ("debian", "archlinux", "notarch"):
            result = self.run_installer(MOCK_ID=distro, MOCK_ID_LIKE="linux debian")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Arch Linux", result.stderr)
        self.assertEqual(self.commands(), [])

    def test_id_like_accepts_arch_derivative(self):
        self.assert_success(self.run_installer("--no-packages", MOCK_ID="derivative", MOCK_ID_LIKE="linux arch"))

    def test_package_plan_preserves_existing_rust(self):
        self.assert_success(self.run_installer())
        privileged = [c for c in self.commands() if c[0] == "sudo"]
        self.assertEqual(privileged, [["sudo", "pacman", "-Syu", "--needed", "base-devel", "git", "python", "grim", "ffmpeg"]])

    def test_missing_rust_tools_add_rust_package(self):
        for missing in ("cargo", "rustc", "cargo rustc"):
            with self.subTest(missing=missing):
                self.log.unlink(missing_ok=True)
                self.assert_success(self.run_installer(MOCK_MISSING=missing))
                packages = next(c for c in self.commands() if c[:2] == ["sudo", "pacman"])
                self.assertEqual(packages[-1], "rust")
                self.assertNotIn("--noconfirm", packages)

    def test_no_packages_checks_dependencies_without_sudo(self):
        for missing in ("git", "python", "grim", "ffmpeg", "cargo", "rustc", "cc", "make"):
            with self.subTest(missing=missing):
                result = self.run_installer("--no-packages", MOCK_MISSING=missing)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Missing prerequisite: " + missing, result.stderr)
        self.assertEqual(self.commands(), [])

    def test_package_manager_prerequisites(self):
        for missing in ("sudo", "pacman"):
            result = self.run_installer(MOCK_MISSING=missing)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(missing, result.stderr)
        self.assertEqual(self.commands(), [])

    def test_no_packages_does_not_require_privileged_tools_without_opt_in(self):
        self.assert_success(self.run_installer(
            "--no-packages", MOCK_MISSING="sudo pacman groupadd usermod modprobe udevadm"
        ))
        self.assertFalse(any(c[0] == "sudo" for c in self.commands()))

    def test_input_prerequisites_fail_before_build_or_system_changes(self):
        for missing in ("sudo", "groupadd", "usermod", "modprobe", "udevadm"):
            result = self.run_installer("--no-packages", "--enable-input", MOCK_MISSING=missing)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Missing prerequisite for --enable-input: " + missing, result.stderr)
        self.assertFalse(any(c[0] in ("sudo", "git") for c in self.commands()))
        self.assertEqual(list(self.system.iterdir()), [])

    def test_install_paths_modes_source_pin_and_cleanup(self):
        prefix = self.home / "prefix with spaces"
        result = self.run_installer("--no-packages", "--prefix", str(prefix))
        self.assert_success(result)
        commands = self.commands()
        self.assertFalse(any(c[0] == "sudo" for c in commands))
        clone = next(c for c in commands if c[0] == "git")
        self.assertEqual(clone[:-1], ["git", "clone", "--depth", "1", "--branch", "v0.2.0", "--single-branch", "--", "https://github.com/AI10xDev/computeruse.git"])
        self.assertIn(["temporary-mode", "700"], commands)
        self.assertIn(["cargo", "install", "--locked", "--path", clone[-1], "--root", str(prefix)], commands)
        launcher = prefix / "bin/computeruse-wayland"
        guide = prefix / "share/doc/computeruse/ARCH-WAYLAND.md"
        self.assertEqual(launcher.stat().st_mode & 0o777, 0o755)
        self.assertEqual(guide.stat().st_mode & 0o777, 0o644)
        self.assertEqual({p.relative_to(prefix).as_posix() for p in prefix.rglob("*") if p.is_file()}, {"bin/computeruse-wayland", "share/doc/computeruse/ARCH-WAYLAND.md"})
        self.assertEqual(list(self.system.iterdir()), [])

    def test_default_prefix(self):
        self.assert_success(self.run_installer("--no-packages"))
        self.assertTrue((self.home / ".local/bin/computeruse-wayland").is_file())

    def test_input_is_explicit_and_scoped_to_uinput(self):
        result = self.run_installer("--no-packages", "--enable-input", SUDO_USER="wrong-user")
        self.assert_success(result)
        commands = [c for c in self.commands() if c[0] == "sudo"]
        self.assertEqual(commands[0], ["sudo", "groupadd", "-f", "computeruse"])
        for call, destination in zip(commands[1:3], ["/etc/udev/rules.d/70-computeruse-uinput.rules", "/etc/modules-load.d/computeruse-uinput.conf"]):
            self.assertEqual(call[:10], ["sudo", "install", "-D", "-o", "root", "-g", "root", "-m", "0644", "--"])
            self.assertEqual(call[-1], destination)
        self.assertEqual(commands[3:], [
            ["sudo", "usermod", "-aG", "computeruse", "desktop-user"],
            ["sudo", "modprobe", "uinput"],
            ["sudo", "udevadm", "control", "--reload-rules"],
            ["sudo", "udevadm", "trigger", "--action=add", "--subsystem-match=misc", "--sysname-match=uinput"],
        ])
        rule = (self.system / "70-computeruse-uinput.rules").read_text()
        self.assertEqual(rule, 'SUBSYSTEM=="misc", KERNEL=="uinput", GROUP="computeruse", MODE="0660", OPTIONS+="static_node=uinput"\n')
        self.assertEqual((self.system / "computeruse-uinput.conf").read_text(), "uinput\n")
        self.assertIn("desktop-wide", result.stderr)
        self.assertIn("Log out", result.stdout)
        self.assertIn("Never run the agent as root", result.stdout)

    def test_old_or_broken_toolchains_fail_actionably(self):
        for env in ({"MOCK_VERSION": "1.87.0"}, {"MOCK_VERSION": "unknown"}, {"MOCK_FAIL": "cargo"}):
            result = self.run_installer("--no-packages", **env)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("1.88", result.stderr)
            self.assertFalse(any(c[0] == "git" for c in self.commands()))

    def test_failures_stop_and_clean_up(self):
        for failure, message in (("pacman", "Full system upgrade"), ("git", "release tag"), ("build", "Cargo build/install"), ("python", "Python 3")):
            with self.subTest(failure=failure):
                self.log.unlink(missing_ok=True)
                result = self.run_installer("--enable-input", MOCK_FAIL=failure)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(message, result.stderr)
                self.assertEqual(list(self.builds.iterdir()), [])
                self.assertEqual(list(self.system.iterdir()), [])

    def test_incomplete_release_fails_before_build(self):
        result = self.run_installer("--no-packages", MOCK_RELEASE_MISSING="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing its Wayland launcher", result.stderr)
        self.assertFalse(any(c[:2] == ["cargo", "install"] for c in self.commands()))
        self.assertEqual(list(self.builds.iterdir()), [])

    def test_input_setup_failure_is_not_reported_as_success(self):
        result = self.run_installer("--no-packages", "--enable-input", MOCK_FAIL="groupadd")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("installation failed", result.stderr)
        self.assertNotIn("Installed computeruse", result.stdout)
        self.assertEqual(list(self.builds.iterdir()), [])
        self.assertEqual(list(self.system.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
