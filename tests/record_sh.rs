use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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

fn assert_fragmented_mp4_output_scoping(recording: &str, output: &Path) {
    let (input, outputs) = recording.split_once("-map\n0:v\n").unwrap();
    let (mp4, png) = outputs.split_once(output.to_str().unwrap()).unwrap();
    for option in [
        "-g\n60\n",
        "-movflags\n+empty_moov+default_base_moof+frag_keyframe\n",
        "-flush_packets\n1\n",
    ] {
        assert!(mp4.contains(option), "missing MP4 option: {option}");
        assert!(!input.contains(option), "MP4 option before input: {option}");
    }
    let png_args: Vec<_> = png.trim().lines().collect();
    assert_eq!(png_args.len(), 7);
    assert_eq!(
        &png_args[..6],
        &["-map", "0:v", "-vf", "fps=2", "-atomic_writing", "1"]
    );
    assert!(png_args[6].ends_with("/frame-%09d.png"));
}

#[test]
fn auto_falls_back_only_during_preflight_and_preserves_output_scoping() {
    let sandbox = Sandbox::new("fallback");
    install_common_fakes(
        &sandbox,
        r#"#!/bin/sh
printf '%s\n' '---' "$@" >> "$TEST_ROOT/ffmpeg.log"
previous=
last=
for argument in "$@"; do
  last=$argument
  if [ "$previous" = "-f" ] && [ "$argument" = "lavfi" ]; then
    printf '%s\n' 'simulated VA-API initialization failure' >&2
    exit 1
  fi
  previous=$argument
done
: > "$TEST_ROOT/recording.mp4"
frame=$(printf '%s' "$last" | sed 's/%09d/000000001/')
printf '%s' 'png' > "$frame"
while [ ! -e "$TEST_ROOT/agent-finished" ]; do sleep 0.05; done
printf '%s' 'mp4' > "$TEST_ROOT/recording.mp4"
sleep 0.2
"#,
        "#!/bin/sh\nprintf '%s\n' \"$@\" > \"$TEST_ROOT/agent.log\"\ntouch \"$TEST_ROOT/agent-finished\"\n",
    );
    let device = sandbox.path("renderD128");
    fs::write(&device, []).unwrap();

    let mut command = Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("record.sh"));
    let instructions = sandbox.path("instructions.txt");
    fs::write(&instructions, "test").unwrap();
    command
        .arg(sandbox.path("recording.mp4"))
        .arg(&instructions)
        .arg("--model")
        .arg("stub")
        .env("PATH", prepend_fake_path(&sandbox.0))
        .env("COMPUTERUSE_VIDEO_ACCEL", "auto")
        .env("COMPUTERUSE_VAAPI_DEVICE", &device)
        .env("COMPUTERUSE_BIN", sandbox.path("agent"))
        .env("COMPUTERUSE_CREDENTIALS", sandbox.path("missing"))
        .env("DISPLAY", ":test")
        .env("TMPDIR", &sandbox.0)
        .env("TEST_ROOT", &sandbox.0);
    let result = command.output().unwrap();

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let log = fs::read_to_string(sandbox.path("ffmpeg.log")).unwrap();
    assert_eq!(log.matches("---").count(), 2);
    let recording_invocation = log.rsplit("---").next().unwrap();
    let output = sandbox.path("recording.mp4");
    assert_fragmented_mp4_output_scoping(recording_invocation, &output);
    let (_, outputs) = recording_invocation.split_once("-map\n0:v\n").unwrap();
    let (mp4, _) = outputs.split_once(output.to_str().unwrap()).unwrap();
    assert!(mp4.contains("-pix_fmt\nyuv420p\n"));
    assert!(!recording_invocation.contains("yuv444p"));
    assert!(recording_invocation.contains("libx264"));
    assert!(recording_invocation.contains("fps=2"));
    assert!(recording_invocation.contains("atomic_writing\n1"));
    assert!(!recording_invocation.contains("h264_vaapi"));
    let agent_log = fs::read_to_string(sandbox.path("agent.log")).unwrap();
    assert!(agent_log.contains("--model\nstub"));
}

#[test]
fn vaapi_options_are_scoped_to_the_recording_output() {
    let sandbox = Sandbox::new("vaapi-scoping");
    install_common_fakes(
        &sandbox,
        r#"#!/bin/sh
printf '%s\n' '---' "$@" >> "$TEST_ROOT/ffmpeg.log"
previous=
last=
for argument in "$@"; do
  last=$argument
  if [ "$previous" = "-f" ] && [ "$argument" = "lavfi" ]; then exit 0; fi
  previous=$argument
done
: > "$TEST_ROOT/recording.mp4"
frame=$(printf '%s' "$last" | sed 's/%09d/000000001/')
printf '%s' 'png' > "$frame"
while [ ! -e "$TEST_ROOT/agent-finished" ]; do sleep 0.05; done
printf '%s' 'mp4' > "$TEST_ROOT/recording.mp4"
sleep 0.2
"#,
        "#!/bin/sh\ntouch \"$TEST_ROOT/agent-finished\"\n",
    );
    let device = sandbox.path("renderD128");
    let instructions = sandbox.path("instructions.txt");
    let output = sandbox.path("recording.mp4");
    fs::write(&device, []).unwrap();
    fs::write(&instructions, "test").unwrap();

    let result = Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("record.sh"))
        .arg(&output)
        .arg(&instructions)
        .env("PATH", prepend_fake_path(&sandbox.0))
        .env("COMPUTERUSE_VIDEO_ACCEL", "vaapi")
        .env("COMPUTERUSE_VAAPI_DEVICE", &device)
        .env("COMPUTERUSE_BIN", sandbox.path("agent"))
        .env("COMPUTERUSE_CREDENTIALS", sandbox.path("missing"))
        .env("TMPDIR", &sandbox.0)
        .env("TEST_ROOT", &sandbox.0)
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let log = fs::read_to_string(sandbox.path("ffmpeg.log")).unwrap();
    let recording = log.rsplit("---").next().unwrap();
    assert_fragmented_mp4_output_scoping(recording, &output);
    let upload = recording.find("format=nv12,hwupload").unwrap();
    let encoder = recording.find("h264_vaapi").unwrap();
    let mp4 = recording.find(output.to_str().unwrap()).unwrap();
    let observations = recording.find("fps=2").unwrap();
    let atomic_writing = recording.find("atomic_writing\n1").unwrap();
    assert!(
        upload < encoder && encoder < mp4 && mp4 < observations && observations < atomic_writing
    );
    assert!(!recording.contains("libx264"));
    assert!(!recording.contains("veryfast"));
    assert!(!recording.contains("-pix_fmt"));
}

#[test]
fn forced_vaapi_reports_probe_failure_without_starting_agent() {
    let sandbox = Sandbox::new("forced");
    install_common_fakes(
        &sandbox,
        "#!/bin/sh\nprintf '%s\n' 'probe rejected device' >&2\nexit 1\n",
        "#!/bin/sh\ntouch \"$TEST_ROOT/agent-started\"\n",
    );
    let device = sandbox.path("renderD129");
    fs::write(&device, []).unwrap();
    let mut command = Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("record.sh"));
    let instructions = sandbox.path("instructions.txt");
    fs::write(&instructions, "test").unwrap();
    let result = command
        .arg(sandbox.path("recording.mp4"))
        .arg(instructions)
        .env("PATH", prepend_fake_path(&sandbox.0))
        .env("COMPUTERUSE_VIDEO_ACCEL", "vaapi")
        .env("COMPUTERUSE_VAAPI_DEVICE", &device)
        .env("COMPUTERUSE_BIN", sandbox.path("agent"))
        .env("TEST_ROOT", &sandbox.0)
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("no device passed preflight"));
    assert!(!sandbox.path("agent-started").exists());
}

#[test]
fn invalid_mode_is_rejected_before_tools_are_started() {
    let sandbox = Sandbox::new("invalid");
    let instructions = sandbox.path("instructions.txt");
    fs::write(&instructions, "test").unwrap();
    let result = Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("record.sh"))
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
    let result = Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("record.sh"))
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
fn capture_failure_stops_the_running_agent() {
    let sandbox = Sandbox::new("capture-failure");
    install_common_fakes(
        &sandbox,
        r#"#!/bin/sh
last=
for argument in "$@"; do last=$argument; done
: > "$TEST_ROOT/recording.mp4"
frame=$(printf '%s' "$last" | sed 's/%09d/000000001/')
printf '%s' 'png' > "$frame"
sleep 0.3
exit 7
"#,
        r#"#!/bin/sh
trap 'touch "$TEST_ROOT/agent-stopped"; exit 0' TERM
touch "$TEST_ROOT/agent-started"
while :; do sleep 0.05; done
"#,
    );
    let instructions = sandbox.path("instructions.txt");
    fs::write(&instructions, "test").unwrap();

    let result = Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("record.sh"))
        .arg(sandbox.path("recording.mp4"))
        .arg(&instructions)
        .env("PATH", prepend_fake_path(&sandbox.0))
        .env("COMPUTERUSE_VIDEO_ACCEL", "off")
        .env("COMPUTERUSE_BIN", sandbox.path("agent"))
        .env("COMPUTERUSE_CREDENTIALS", sandbox.path("missing"))
        .env("TMPDIR", &sandbox.0)
        .env("TEST_ROOT", &sandbox.0)
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert!(sandbox.path("agent-started").is_file());
    assert!(sandbox.path("agent-stopped").is_file());
    assert!(String::from_utf8_lossy(&result.stderr).contains("status 7"));
}
