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

fn record_command() -> Command {
    let mut command = Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("record.sh"));
    command.env_remove("COMPUTERUSE_OBSERVATION_FPS");
    command
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
for number in $(seq 1 25); do
  timestamp=$(printf '%020d' "$((1789171200000000 + number))")
  frame=$(printf '%s' "$last" | sed "s/%020d/$timestamp/")
  printf '%s' 'png' > "$frame"
done
while [ ! -e "$TEST_ROOT/agent-finished" ]; do sleep 0.05; done
printf '%s' 'mp4' > "$TEST_ROOT/recording.mp4"
sleep 0.2
"#,
        r#"#!/bin/sh
printf '%s\n' "$@" > "$TEST_ROOT/agent.log"
frames=$3
for attempt in $(seq 1 100); do
  set -- "$frames"/frame-*.png
  if [ "$#" -eq 20 ]; then
    printf '%s\n' "$@" > "$TEST_ROOT/retained-frames"
    touch "$TEST_ROOT/agent-finished"
    exit 0
  fi
  sleep 0.05
done
exit 1
"#,
    );
    let device = sandbox.path("renderD128");
    fs::write(&device, []).unwrap();

    let mut command = record_command();
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
    let (video, observations) = recording_invocation
        .split_once(output.to_str().unwrap())
        .unwrap();
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
    let image_args: Vec<_> = observations.lines().skip(1).collect();
    assert_eq!(
        &image_args[..image_args.len() - 1],
        [
            "-map",
            "0:v",
            "-vf",
            "select='isnan(prev_selected_t)+gt(floor(t*10),floor(prev_selected_t*10))'",
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
    assert!(
        image_args
            .last()
            .unwrap()
            .ends_with("/frame-capture-%020d.png")
    );
    assert!(!recording_invocation.contains("h264_vaapi"));
    assert!(
        String::from_utf8_lossy(&result.stdout)
            .contains("at 30 FPS and sampling agent observations at 10 FPS.")
    );
    let agent_log = fs::read_to_string(sandbox.path("agent.log")).unwrap();
    assert!(agent_log.contains("--model\nstub"));
    let retained = fs::read_to_string(sandbox.path("retained-frames")).unwrap();
    let retained: Vec<_> = retained.lines().collect();
    assert_eq!(retained.len(), 20);
    assert!(retained[0].ends_with("frame-capture-00001789171200000006.png"));
    assert!(retained[19].ends_with("frame-capture-00001789171200000025.png"));
}

#[test]
fn vaapi_options_are_scoped_to_the_recording_output() {
    for fps in ["1", "17", "30"] {
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
frame=$(printf '%s' "$last" | sed 's/%020d/00001789171200123456/')
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

        let result = record_command()
            .arg(&output)
            .arg(&instructions)
            .env("PATH", prepend_fake_path(&sandbox.0))
            .env("COMPUTERUSE_VIDEO_ACCEL", "vaapi")
            .env("COMPUTERUSE_OBSERVATION_FPS", fps)
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
        let upload = recording.find("format=nv12,hwupload").unwrap();
        let encoder = recording.find("h264_vaapi").unwrap();
        let mp4 = recording.find(output.to_str().unwrap()).unwrap();
        assert!(upload < encoder && encoder < mp4);
        let (video, observations) = recording.split_once(output.to_str().unwrap()).unwrap();
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
        let filter = format!(
            "select='isnan(prev_selected_t)+gt(floor(t*{fps}),floor(prev_selected_t*{fps}))'"
        );
        let image_args: Vec<_> = observations.lines().skip(1).collect();
        assert_eq!(
            &image_args[..image_args.len() - 1],
            [
                "-map",
                "0:v",
                "-vf",
                &filter,
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
        assert!(
            image_args
                .last()
                .unwrap()
                .ends_with("/frame-capture-%020d.png")
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains(&format!(
            "at 30 FPS and sampling agent observations at {fps} FPS."
        )));
        assert!(!recording.contains("libx264"));
        assert!(!recording.contains("veryfast"));
    }
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
    let mut command = record_command();
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
fn capture_failure_stops_the_running_agent() {
    let sandbox = Sandbox::new("capture-failure");
    install_common_fakes(
        &sandbox,
        r#"#!/bin/sh
last=
for argument in "$@"; do last=$argument; done
: > "$TEST_ROOT/recording.mp4"
frame=$(printf '%s' "$last" | sed 's/%020d/00001789171200123456/')
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

    let result = record_command()
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
args=()
while (( $# )); do
  case "$1" in
    -f) args+=(-f lavfi); shift 2 ;;
    -framerate|-video_size) shift 2 ;;
    -i)
      args+=(-i 'testsrc2=size=160x120:rate=30:duration=30,realtime=speed=2,settb=1/1000000,setpts=PTS+1789171200123456')
      shift 2 ;;
    *) args+=("$1"); shift ;;
  esac
done
printf '%s' "$$" > "$TEST_ROOT/ffmpeg-pid"
exec "$REAL_FFMPEG" "${args[@]}"
"#,
        r#"#!/bin/bash
frames=$3
for (( attempt = 0; attempt < 200; attempt++ )); do
  files=("$frames"/frame-capture-*.png)
  latest=${files[-1]##*/frame-capture-}
  latest=${latest%.png}
  if [[ "$latest" =~ ^[0-9]{20}$ ]] && (( 10#$latest >= 1789171206123456 )); then
    kill -KILL "$(< "$TEST_ROOT/ffmpeg-pid")"
    printf '%s\n' "${files[@]##*/}" > "$TEST_ROOT/capture-names"
    # Let record.sh observe the recorder failure rather than normal agent completion.
    while :; do sleep 0.05; done
  fi
  sleep 0.05
done
exit 1
"#,
    );
    let instructions = sandbox.path("instructions.txt");
    let output = sandbox.path("recording.mp4");
    fs::write(&instructions, "test").unwrap();
    let result = record_command()
        .arg(&output)
        .arg(instructions)
        .env("PATH", prepend_fake_path(&sandbox.0))
        .env("REAL_FFMPEG", ffmpeg.trim())
        .env("COMPUTERUSE_VIDEO_ACCEL", "off")
        .env("COMPUTERUSE_OBSERVATION_FPS", "17")
        .env("COMPUTERUSE_BIN", sandbox.path("agent"))
        .env("COMPUTERUSE_CREDENTIALS", sandbox.path("missing"))
        .env("TMPDIR", &sandbox.0)
        .env("TEST_ROOT", &sandbox.0)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success(), "{stderr}");
    assert!(stderr.contains("status 137"), "{stderr}");

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
    for timestamp in &timestamps {
        // These historical input PTS must survive, not be replaced by write time
        // or rounded to the 17 FPS sampling grid (including subsecond precision).
        assert!((0..900).any(|n| *timestamp == 1_789_171_200_123_456 + (n * 1_000_000 + 15) / 30));
    }
    for pair in timestamps.windows(2) {
        assert_eq!(pair[1] * 17 / 1_000_000, pair[0] * 17 / 1_000_000 + 1);
    }

    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=start_time,duration:packet=pts_time,flags",
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
        "Recovered MP4: {stream}; keyframes: {keyframes:?}; capture range: {:?}..{:?}",
        timestamps.first(),
        timestamps.last()
    );
}
