use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct Sandbox(PathBuf);

impl Sandbox {
    fn new(name: &str) -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("computeruse-record-{name}-{suffix}"));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn executable(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, contents).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn install_common_fakes(sandbox: &Sandbox, ffmpeg: &str, agent: &str) {
    sandbox.executable("ffmpeg", ffmpeg);
    sandbox.executable(
        "xdpyinfo",
        "#!/bin/sh\nprintf '%s\n' '  dimensions:    1280x720 pixels'\n",
    );
    sandbox.executable("timeout", "#!/bin/sh\nshift\nexec \"$@\"\n");
    sandbox.executable("agent", agent);
}

fn prepend_fake_path(command_path: &Path) -> String {
    format!(
        "{}:{}",
        command_path.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

fn record_command() -> Command {
    let mut command = Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("record.sh"));
    command.env_remove("COMPUTERUSE_OBSERVATION_FPS");
    command
}

fn record_with_fakes(sandbox: &Sandbox) -> Command {
    let instructions = sandbox.path("instructions.txt");
    fs::write(&instructions, "test").unwrap();
    let mut command = record_command();
    command
        .arg(sandbox.path("recording.mp4"))
        .arg(instructions)
        .env("PATH", prepend_fake_path(&sandbox.0))
        .env("COMPUTERUSE_VIDEO_ACCEL", "off")
        .env("COMPUTERUSE_BIN", sandbox.path("agent"))
        .env("COMPUTERUSE_CREDENTIALS", sandbox.path("missing"))
        .env("DISPLAY", ":test")
        .env("TMPDIR", &sandbox.0)
        .env("TEST_ROOT", &sandbox.0);
    command
}

// Background shells inherit ignored SIGINT. Reset it so the fake can model
// FFmpeg's interrupt handler without waiting for the five-second TERM fallback.
const CAPTURE_FFMPEG: &str = r#"#!/usr/bin/env -S --default-signal=INT /bin/bash
last=${!#}
if [[ "$last" == - ]]; then
  printf '%s\n' '---' "$@" >> "$TEST_ROOT/probe.log"
  if [[ "${TEST_PROBE_FAIL:-}" == 1 ]]; then
    printf '%s\n' 'simulated VA-API initialization failure' >&2
    exit 1
  fi
  exit 0
fi
role=recording
if [[ "$last" == *.png ]]; then role=observation; fi
printf '%s\n' '---' "$@" >> "$TEST_ROOT/$role.log"
stop() {
  printf '%s\n' "$1" > "$TEST_ROOT/$role-stopped"
  if [[ "$role" == recording ]]; then printf '%s' 'finalized mp4' > "$last"; fi
  exit 0
}
trap 'stop INT' INT
trap 'stop TERM' TERM
if [[ "$role" == recording ]]; then
  if [[ "${TEST_HOLD_OUTPUT:-}" == recording ]]; then
    : > "$last"
  else
    printf '%s' 'partial mp4' > "$last"
  fi
else
  printf '%s' "${last%/*}" > "$TEST_ROOT/frames-path"
  frame=${last/\%020d/00001789171200123456}
  if [[ "${TEST_FAIL_PHASE:-}" == before ]]; then
    :
  elif [[ "${TEST_HOLD_OUTPUT:-}" == observation ]]; then
    : > "$frame"
    printf '%s' 'png' > "$frame.tmp"
  else
    for (( number = 1; number <= ${TEST_FRAME_COUNT:-1}; number++ )); do
      printf -v timestamp '%020d' "$((1789171200000000 + number))"
      printf '%s' 'png' > "${last/\%020d/$timestamp}"
    done
  fi
fi
printf '%s\n' "$$" > "$TEST_ROOT/$role.pid"
touch "$TEST_ROOT/$role-ready"
if [[ "$role" == "${TEST_HOLD_OUTPUT:-}" ]]; then
  while [[ ! -e "$TEST_ROOT/release-outputs" ]]; do sleep 0.01; done
  if [[ "$role" == recording ]]; then
    printf '%s' 'partial mp4' > "$last"
  else
    mv "$frame.tmp" "$frame"
  fi
fi
if [[ "$role" == "${TEST_FAIL_ROLE:-}" ]]; then
  if [[ "$TEST_FAIL_PHASE" == before ]]; then
    while [[ ! -s "$TEST_ROOT/recording.pid" || ! -s "$TEST_ROOT/observation.pid" ]]; do
      sleep 0.01
    done
  else
    while [[ ! -e "$TEST_ROOT/agent-started" ]]; do sleep 0.01; done
  fi
  exit "$TEST_FAIL_STATUS"
fi
while :; do sleep 0.05; done
"#;

const CAPTURE_AGENT: &str = r#"#!/bin/bash
trap 'printf "%s\n" TERM > "$TEST_ROOT/agent-stopped"; exit 0' TERM
printf '%s\n' "$$" > "$TEST_ROOT/agent.pid"
printf '%s\n' "$@" > "$TEST_ROOT/agent.log"
touch "$TEST_ROOT/agent-started"
if [[ "${TEST_AGENT_WAIT:-}" == 1 ]]; then
  while :; do sleep 0.05; done
fi
exit "${TEST_AGENT_STATUS:-0}"
"#;

fn assert_observation_args(sandbox: &Sandbox, fps: &str) {
    let log = fs::read_to_string(sandbox.path("observation.log")).unwrap();
    assert_eq!(log.matches("---\n").count(), 1);
    let args: Vec<_> = log.lines().skip(1).collect();
    assert_eq!(
        &args[..args.len() - 1],
        [
            "-hide_banner",
            "-nostdin",
            "-n",
            "-copyts",
            "-f",
            "x11grab",
            "-framerate",
            fps,
            "-video_size",
            "1280x720",
            "-i",
            ":test",
            "-map",
            "0:v",
            "-fps_mode",
            "passthrough",
            "-enc_time_base",
            "1:1000000",
            "-compression_level",
            "1",
            "-frame_pts",
            "1",
            "-atomic_writing",
            "1"
        ]
    );
    assert!(args.last().unwrap().ends_with("/frame-capture-%020d.png"));
}

fn assert_capture_cleanup(sandbox: &Sandbox) {
    let mut pids = Vec::new();
    for role in ["recording", "observation", "agent"] {
        let path = sandbox.path(&format!("{role}.pid"));
        if role == "agent" && !path.exists() {
            continue;
        }
        let pid = fs::read_to_string(path).unwrap();
        let pid: u32 = pid.trim().parse().unwrap();
        assert!(!pids.contains(&pid), "{role} reused another worker's PID");
        pids.push(pid);
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .unwrap();
        assert!(!alive.status.success(), "{role} process {pid} was orphaned");
    }
    let frames = fs::read_to_string(sandbox.path("frames-path")).unwrap();
    assert!(
        !Path::new(&frames).exists(),
        "capture frames were not removed"
    );
    assert!(fs::metadata(sandbox.path("recording.mp4")).unwrap().len() > 0);
}

fn wait_for_file(path: &Path) -> bool {
    for _ in 0..200 {
        if path.is_file() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn auto_falls_back_only_during_preflight_and_preserves_output_scoping() {
    let sandbox = Sandbox::new("fallback");
    install_common_fakes(
        &sandbox,
        CAPTURE_FFMPEG,
        r#"#!/bin/sh
printf '%s\n' "$$" > "$TEST_ROOT/agent.pid"
printf '%s\n' "$@" > "$TEST_ROOT/agent.log"
frames=$3
for attempt in $(seq 1 100); do
  set -- "$frames"/frame-*.png
  if [ "$#" -eq 20 ]; then
    printf '%s\n' "$@" > "$TEST_ROOT/retained-frames"
    exit 0
  fi
  sleep 0.05
done
exit 1
"#,
    );
    let device = sandbox.path("renderD128");
    fs::write(&device, []).unwrap();

    let result = record_with_fakes(&sandbox)
        .arg("--model")
        .arg("stub")
        .env("COMPUTERUSE_VIDEO_ACCEL", "auto")
        .env("COMPUTERUSE_VAAPI_DEVICE", &device)
        .env("TEST_PROBE_FAIL", "1")
        .env("TEST_FRAME_COUNT", "25")
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let probe = fs::read_to_string(sandbox.path("probe.log")).unwrap();
    assert_eq!(probe.matches("---\n").count(), 1);
    let recording = fs::read_to_string(sandbox.path("recording.log")).unwrap();
    assert_eq!(recording.matches("---\n").count(), 1);
    let output = sandbox.path("recording.mp4");
    let (video, trailing) = recording.split_once(output.to_str().unwrap()).unwrap();
    assert_eq!(trailing, "\n");
    assert!(video.contains("-framerate\n30\n-video_size\n1280x720\n"));
    assert!(video.contains("-copyts\n"));
    assert!(!video.contains("-start_at_zero"));
    assert!(!video.contains("-use_wallclock_as_timestamps"));
    assert!(video.contains("-vf\nsetpts=PTS-STARTPTS\n-c:v\nlibx264\n-preset\nveryfast\n"));
    assert!(video.ends_with(
        "-g\n60\n-bf\n0\n-movflags\n+frag_keyframe+empty_moov+default_base_moof\n-flush_packets\n1\n"
    ));
    assert!(!video.contains("compression_level"));
    assert!(!video.contains("fps="));
    assert!(!recording.contains("h264_vaapi"));
    assert!(!recording.contains("-vaapi_device"));
    assert_observation_args(&sandbox, "10");
    assert!(String::from_utf8_lossy(&result.stderr).contains("falling back to software encoding"));
    assert!(
        String::from_utf8_lossy(&result.stdout)
            .contains("at 30 FPS and capturing agent observations separately at 10 FPS.")
    );
    let agent_log = fs::read_to_string(sandbox.path("agent.log")).unwrap();
    assert!(agent_log.contains("--model\nstub"));
    let retained = fs::read_to_string(sandbox.path("retained-frames")).unwrap();
    let retained: Vec<_> = retained.lines().collect();
    assert_eq!(retained.len(), 20);
    assert!(retained[0].ends_with("frame-capture-00001789171200000006.png"));
    assert!(retained[19].ends_with("frame-capture-00001789171200000025.png"));
    assert_capture_cleanup(&sandbox);
}

#[test]
fn vaapi_options_are_scoped_to_the_recording_output() {
    for fps in ["1", "17", "30"] {
        let sandbox = Sandbox::new("vaapi-scoping");
        install_common_fakes(&sandbox, CAPTURE_FFMPEG, CAPTURE_AGENT);
        let device = sandbox.path("renderD128");
        let output = sandbox.path("recording.mp4");
        fs::write(&device, []).unwrap();

        let result = record_with_fakes(&sandbox)
            .env("COMPUTERUSE_VIDEO_ACCEL", "vaapi")
            .env("COMPUTERUSE_OBSERVATION_FPS", fps)
            .env("COMPUTERUSE_VAAPI_DEVICE", &device)
            .output()
            .unwrap();

        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let probe = fs::read_to_string(sandbox.path("probe.log")).unwrap();
        assert_eq!(probe.matches("---\n").count(), 1);
        let recording = fs::read_to_string(sandbox.path("recording.log")).unwrap();
        assert_eq!(recording.matches("---\n").count(), 1);
        let upload = recording.find("format=nv12,hwupload").unwrap();
        let encoder = recording.find("h264_vaapi").unwrap();
        let mp4 = recording.find(output.to_str().unwrap()).unwrap();
        assert!(upload < encoder && encoder < mp4);
        let (video, trailing) = recording.split_once(output.to_str().unwrap()).unwrap();
        assert_eq!(trailing, "\n");
        assert!(video.contains(&format!("-vaapi_device\n{}\n", device.display())));
        assert!(video.contains("-framerate\n30\n-video_size\n1280x720\n"));
        assert!(video.contains("-copyts\n"));
        assert!(!video.contains("-start_at_zero"));
        assert!(video.contains(
            "-vf\nsetpts=PTS-STARTPTS,format=nv12,hwupload\n-c:v\nh264_vaapi\n-qp\n23\n"
        ));
        assert!(video.ends_with(
            "-g\n60\n-bf\n0\n-movflags\n+frag_keyframe+empty_moov+default_base_moof\n-flush_packets\n1\n"
        ));
        assert!(!video.contains("compression_level"));
        assert!(!video.contains("fps="));
        assert_observation_args(&sandbox, fps);
        assert!(String::from_utf8_lossy(&result.stdout).contains(&format!(
            "at 30 FPS and capturing agent observations separately at {fps} FPS."
        )));
        assert!(!recording.contains("libx264"));
        assert!(!recording.contains("veryfast"));
        assert_capture_cleanup(&sandbox);
    }
}

#[test]
fn forced_vaapi_reports_probe_failure_without_starting_agent() {
    let sandbox = Sandbox::new("forced");
    install_common_fakes(&sandbox, CAPTURE_FFMPEG, CAPTURE_AGENT);
    let device = sandbox.path("renderD129");
    fs::write(&device, []).unwrap();
    let result = record_with_fakes(&sandbox)
        .env("COMPUTERUSE_VIDEO_ACCEL", "vaapi")
        .env("COMPUTERUSE_VAAPI_DEVICE", &device)
        .env("TEST_PROBE_FAIL", "1")
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("no device passed preflight"));
    let probe = fs::read_to_string(sandbox.path("probe.log")).unwrap();
    assert_eq!(probe.matches("---\n").count(), 1);
    assert!(!sandbox.path("recording.log").exists());
    assert!(!sandbox.path("observation.log").exists());
    assert!(!sandbox.path("recording.mp4").exists());
    assert!(!sandbox.path("agent-started").exists());
}

#[test]
fn invalid_mode_is_rejected_before_tools_are_started() {
    let sandbox = Sandbox::new("invalid");
    let instructions = sandbox.path("instructions.txt");
    fs::write(&instructions, "test").unwrap();
    let result = record_command()
        .arg(sandbox.path("recording.mp4"))
        .arg(instructions)
        .env("COMPUTERUSE_VIDEO_ACCEL", "cuda")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("expected auto, vaapi, or off"));
}

#[test]
fn existing_recording_is_never_overwritten() {
    let sandbox = Sandbox::new("overwrite");
    install_common_fakes(&sandbox, "#!/bin/sh\nexit 99\n", "#!/bin/sh\nexit 99\n");
    let instructions = sandbox.path("instructions.txt");
    let output = sandbox.path("recording.mp4");
    fs::write(&instructions, "test").unwrap();
    fs::write(&output, "existing").unwrap();
    let result = record_command()
        .arg(&output)
        .arg(instructions)
        .env("PATH", prepend_fake_path(&sandbox.0))
        .env("COMPUTERUSE_VIDEO_ACCEL", "off")
        .env("COMPUTERUSE_BIN", sandbox.path("agent"))
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(fs::read_to_string(output).unwrap(), "existing");
}

#[test]
fn capture_failures_report_role_status_and_phase_and_stop_children() {
    for (phase, diagnostic) in [
        ("before", "before capture was ready"),
        ("running", "while the agent was running"),
    ] {
        for role in ["recording", "observation"] {
            for status in ["7", "0"] {
                let sandbox = Sandbox::new("capture-failure");
                install_common_fakes(&sandbox, CAPTURE_FFMPEG, CAPTURE_AGENT);
                let device = sandbox.path("renderD128");
                fs::write(&device, []).unwrap();
                let result = record_with_fakes(&sandbox)
                    .env("COMPUTERUSE_VIDEO_ACCEL", "auto")
                    .env("COMPUTERUSE_VAAPI_DEVICE", &device)
                    .env("TEST_FAIL_ROLE", role)
                    .env("TEST_FAIL_PHASE", phase)
                    .env("TEST_FAIL_STATUS", status)
                    .env("TEST_AGENT_WAIT", "1")
                    .output()
                    .unwrap();

                let stderr = String::from_utf8_lossy(&result.stderr);
                assert_eq!(
                    result.status.code(),
                    Some(1),
                    "{role}/{phase}/{status}: {stderr}"
                );
                assert!(stderr.contains(&format!(
                    "FFmpeg {role} process exited with status {status} {diagnostic}; the partial recording was retained."
                )), "{stderr}");
                assert!(!stderr.contains("falling back"), "{stderr}");
                for worker in ["probe", "recording", "observation"] {
                    let log = fs::read_to_string(sandbox.path(&format!("{worker}.log"))).unwrap();
                    assert_eq!(log.matches("---\n").count(), 1, "{worker} was restarted");
                }
                assert_eq!(sandbox.path("agent-started").exists(), phase == "running");
                assert_eq!(sandbox.path("agent-stopped").exists(), phase == "running");
                if phase == "running" {
                    assert_eq!(
                        fs::read_to_string(sandbox.path("agent-stopped")).unwrap(),
                        "TERM\n"
                    );
                }
                let (survivor, signal) = if role == "recording" {
                    ("observation", "TERM\n")
                } else {
                    ("recording", "INT\n")
                };
                assert_eq!(
                    fs::read_to_string(sandbox.path(&format!("{survivor}-stopped"))).unwrap(),
                    signal
                );
                assert_capture_cleanup(&sandbox);
            }
        }
    }
}

#[test]
fn agent_success_and_failure_finalize_recording_and_stop_both_captures() {
    for status in [0, 23] {
        let sandbox = Sandbox::new("agent-exit");
        install_common_fakes(&sandbox, CAPTURE_FFMPEG, CAPTURE_AGENT);
        let result = record_with_fakes(&sandbox)
            .env("TEST_AGENT_STATUS", status.to_string())
            .output()
            .unwrap();

        let stderr = String::from_utf8_lossy(&result.stderr);
        assert_eq!(result.status.code(), Some(status), "{stderr}");
        if status != 0 {
            assert!(stderr.contains("Agent exited with status 23; the recording was finalized."));
        }
        assert!(!sandbox.path("probe.log").exists());
        assert!(!sandbox.path("agent-stopped").exists());
        assert_eq!(
            fs::read_to_string(sandbox.path("observation-stopped")).unwrap(),
            "TERM\n"
        );
        assert_eq!(
            fs::read_to_string(sandbox.path("recording-stopped")).unwrap(),
            "INT\n"
        );
        assert_eq!(
            fs::read_to_string(sandbox.path("recording.mp4")).unwrap(),
            "finalized mp4"
        );
        assert_capture_cleanup(&sandbox);
    }
}

#[test]
fn external_termination_stops_the_agent_and_both_captures() {
    for (signal, status) in [("-TERM", 143), ("-INT", 130)] {
        let sandbox = Sandbox::new("termination");
        install_common_fakes(&sandbox, CAPTURE_FFMPEG, CAPTURE_AGENT);
        let child = record_with_fakes(&sandbox)
            .env("TEST_AGENT_WAIT", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = wait_for_file(&sandbox.path("agent-started"));
        let signal_result = Command::new("kill")
            .args([signal, &child.id().to_string()])
            .output()
            .unwrap();
        let result = child.wait_with_output().unwrap();

        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(started, "{stderr}");
        assert!(signal_result.status.success());
        assert_eq!(result.status.code(), Some(status), "{stderr}");
        assert_eq!(
            fs::read_to_string(sandbox.path("agent-stopped")).unwrap(),
            "TERM\n"
        );
        assert_eq!(
            fs::read_to_string(sandbox.path("observation-stopped")).unwrap(),
            "TERM\n"
        );
        assert_eq!(
            fs::read_to_string(sandbox.path("recording-stopped")).unwrap(),
            "INT\n"
        );
        assert_capture_cleanup(&sandbox);
    }
}

#[test]
fn startup_requires_nonempty_recording_and_a_complete_nonempty_observation() {
    for held_output in ["recording", "observation"] {
        let sandbox = Sandbox::new("startup-readiness");
        install_common_fakes(&sandbox, CAPTURE_FFMPEG, CAPTURE_AGENT);
        let child = record_with_fakes(&sandbox)
            .env("TEST_HOLD_OUTPUT", held_output)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let staged = wait_for_file(&sandbox.path("recording-ready"))
            && wait_for_file(&sandbox.path("observation-ready"));
        std::thread::sleep(Duration::from_millis(300));
        let started_early = sandbox.path("agent-started").exists();
        // Release the held output before asserting, so a regression cannot orphan workers.
        fs::write(sandbox.path("release-outputs"), []).unwrap();
        let result = child.wait_with_output().unwrap();

        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(staged, "{stderr}");
        assert!(
            !started_early,
            "agent started with incomplete {held_output}"
        );
        assert!(result.status.success(), "{stderr}");
        assert!(sandbox.path("agent-started").exists());
        assert_capture_cleanup(&sandbox);
    }
}

#[test]
fn invalid_observation_fps_is_rejected_before_tools_are_started() {
    let sandbox = Sandbox::new("invalid-fps");
    install_common_fakes(
        &sandbox,
        "#!/bin/sh\ntouch \"$TEST_ROOT/ffmpeg-started\"\nexit 99\n",
        "#!/bin/sh\ntouch \"$TEST_ROOT/agent-started\"\nexit 99\n",
    );
    sandbox.executable(
        "xdpyinfo",
        "#!/bin/sh\ntouch \"$TEST_ROOT/xdpyinfo-started\"\nexit 99\n",
    );
    let instructions = sandbox.path("instructions.txt");
    let device = sandbox.path("renderD128");
    fs::write(&instructions, "test").unwrap();
    fs::write(&device, []).unwrap();

    for fps in [
        "",
        "0",
        "-1",
        "1.5",
        "31",
        "999999999999999999999999999999",
        "abc",
        " 10",
        "10 ",
        "1+1",
    ] {
        let result = record_command()
            .arg(sandbox.path("recording.mp4"))
            .arg(&instructions)
            .env("PATH", prepend_fake_path(&sandbox.0))
            .env("COMPUTERUSE_VIDEO_ACCEL", "auto")
            .env("COMPUTERUSE_VAAPI_DEVICE", &device)
            .env("COMPUTERUSE_OBSERVATION_FPS", fps)
            .env("COMPUTERUSE_BIN", sandbox.path("agent"))
            .env("COMPUTERUSE_CREDENTIALS", sandbox.path("missing"))
            .env("TMPDIR", &sandbox.0)
            .env("TEST_ROOT", &sandbox.0)
            .output()
            .unwrap();

        assert_eq!(result.status.code(), Some(2), "FPS: {fps:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains(&format!(
            "Invalid COMPUTERUSE_OBSERVATION_FPS value '{fps}'; expected a positive integer from 1 to 30."
        )));
        assert!(!sandbox.path("ffmpeg-started").exists());
        assert!(!sandbox.path("xdpyinfo-started").exists());
        assert!(!sandbox.path("agent-started").exists());
    }
}

#[test]
#[ignore = "requires real ffmpeg with libx264 and ffprobe; runs a SIGKILL recovery check"]
fn lavfi_capture_timestamps_and_killed_mp4_are_usable() {
    let sandbox = Sandbox::new("lavfi");
    let ffmpeg = Command::new("sh")
        .args(["-c", "command -v ffmpeg"])
        .output()
        .unwrap();
    assert!(ffmpeg.status.success());
    let ffmpeg = String::from_utf8(ffmpeg.stdout).unwrap();
    install_common_fakes(
        &sandbox,
        // Substitute only the X11 input, exercising the script's actual output options.
        r#"#!/bin/bash
set -euo pipefail
last=${!#}
role=recording
if [[ "$last" == *.png ]]; then
  role=observation
  printf '%s' "${last%/*}" > "$TEST_ROOT/frames-path"
fi
printf '%s\n' '---' "$@" > "$TEST_ROOT/$role.log"
args=()
rate=
while (( $# )); do
  case "$1" in
    -f) args+=(-f lavfi); shift 2 ;;
    -framerate) rate=$2; shift 2 ;;
    -video_size) shift 2 ;;
    -i)
      args+=(-i "testsrc2=size=160x120:rate=$rate:duration=30,realtime=speed=2,settb=1/1000000,setpts=PTS+1789171200123456")
      shift 2 ;;
    *) args+=("$1"); shift ;;
  esac
done
printf '%s' "$rate" > "$TEST_ROOT/$role-rate"
printf '%s' "$$" > "$TEST_ROOT/$role.pid"
exec "$REAL_FFMPEG" "${args[@]}"
"#,
        r#"#!/bin/bash
set -euo pipefail
trap 'touch "$TEST_ROOT/agent-stopped"; exit 0' TERM
printf '%s' "$$" > "$TEST_ROOT/agent.pid"
frames=$3
recording=$(< "$TEST_ROOT/recording.pid")
observation=$(< "$TEST_ROOT/observation.pid")
paused_at=
for (( attempt = 0; attempt < 300; attempt++ )); do
  files=("$frames"/frame-capture-*.png)
  latest=${files[-1]##*/frame-capture-}
  latest=${latest%.png}
  if [[ ! "$latest" =~ ^[0-9]{20}$ ]]; then
    printf 'Unexpected capture filename: %s\n' "${files[-1]}" >&2
    exit 2
  fi
  if [[ "$latest" =~ ^[0-9]{20}$ ]]; then
    if [[ -z "$paused_at" ]] && (( 10#$latest >= 1789171206623456 )); then
      kill -STOP "$recording"
      paused_at=$((10#$latest))
      printf '%s' "$paused_at" > "$TEST_ROOT/capture-at-stop"
    elif [[ -n "$paused_at" ]] && (( 10#$latest >= paused_at + 1000000 )); then
      read -r pid name state rest < "/proc/$recording/stat"
      [[ "$state" == T ]]
      kill -0 "$observation"
      printf '%s' "$((10#$latest))" > "$TEST_ROOT/capture-after-stop"
      printf '%s\n' "${files[@]##*/}" > "$TEST_ROOT/capture-names"
      kill -KILL "$recording"
      # Let record.sh observe the recorder failure rather than normal agent completion.
      while :; do sleep 0.05; done
    fi
  fi
  sleep 0.05
done
exit 1
"#,
    );
    let output = sandbox.path("recording.mp4");
    let result = record_with_fakes(&sandbox)
        .env("REAL_FFMPEG", ffmpeg.trim())
        .env("COMPUTERUSE_OBSERVATION_FPS", "17")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert_eq!(result.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(
            "FFmpeg recording process exited with status 137 while the agent was running"
        ),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(sandbox.path("recording-rate")).unwrap(),
        "30"
    );
    assert_eq!(
        fs::read_to_string(sandbox.path("observation-rate")).unwrap(),
        "17"
    );
    assert_observation_args(&sandbox, "17");
    assert!(sandbox.path("agent-stopped").exists());
    assert_capture_cleanup(&sandbox);
    let paused_at: u64 = fs::read_to_string(sandbox.path("capture-at-stop"))
        .unwrap()
        .parse()
        .unwrap();
    let after_stop: u64 = fs::read_to_string(sandbox.path("capture-after-stop"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(after_stop - paused_at >= 1_000_000);

    let names = fs::read_to_string(sandbox.path("capture-names")).unwrap();
    let timestamps: Vec<u64> = names
        .lines()
        .map(|name| {
            let digits = name
                .strip_prefix("frame-capture-")
                .unwrap()
                .strip_suffix(".png")
                .unwrap();
            assert_eq!(digits.len(), 20);
            digits.parse().unwrap()
        })
        .collect();
    assert!(timestamps.len() >= 20);
    // Preserve the independently captured 17 FPS source's exact historical PTS,
    // including its offset, rather than publication time or a rebased/resampled grid.
    let source_frames: Vec<_> = timestamps
        .iter()
        .map(|timestamp| {
            (0..510)
                .find(|n| *timestamp == 1_789_171_200_123_456 + (n * 1_000_000 + 8) / 17)
                .unwrap_or_else(|| panic!("observation PTS {timestamp} was not in the source"))
        })
        .collect();
    assert!(source_frames.windows(2).all(|pair| pair[1] == pair[0] + 1));

    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=start_time,duration,r_frame_rate,avg_frame_rate:packet=pts_time,flags",
            "-of",
            "json",
        ])
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        probe.status.success(),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let probe: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    let stream = &probe["streams"][0];
    assert_eq!(stream["start_time"], "0.000000");
    assert_eq!(stream["r_frame_rate"], "30/1");
    assert_eq!(stream["avg_frame_rate"], "30/1");
    assert!(stream["duration"].as_str().unwrap().parse::<f64>().unwrap() >= 4.0);
    let keyframes: Vec<f64> = probe["packets"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|packet| packet["flags"].as_str().unwrap().contains('K'))
        .map(|packet| packet["pts_time"].as_str().unwrap().parse().unwrap())
        .collect();
    assert!(keyframes.len() >= 2);
    assert_eq!(keyframes[0], 0.0);
    assert!(keyframes.windows(2).all(|pair| pair[1] - pair[0] <= 2.001));
    let decode = Command::new(ffmpeg.trim())
        .args(["-v", "error", "-xerror", "-i"])
        .arg(&output)
        .args(["-f", "null", "-"])
        .output()
        .unwrap();
    assert!(
        decode.status.success(),
        "{}",
        String::from_utf8_lossy(&decode.stderr)
    );
    eprintln!(
        "Recovered MP4: {stream}; keyframes: {keyframes:?}; capture range: {:?}..{:?}; observations while recording was stopped: {paused_at}..{after_stop}",
        timestamps.first(),
        timestamps.last()
    );
}
