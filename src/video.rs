use anyhow::{Context, Result, anyhow, bail};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FRAME_PREFIX: &str = "frame-";
const FRAME_SUFFIX: &str = ".png";

#[derive(Clone, Copy, Debug, Default)]
struct FrameSelection {
    frames_per_second: Option<u32>,
    max_frames: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccelerationMode {
    Auto,
    Vaapi,
    Off,
}

impl AccelerationMode {
    fn parse(value: Option<&str>) -> Result<Self> {
        match value.unwrap_or("auto") {
            "auto" => Ok(Self::Auto),
            "vaapi" => Ok(Self::Vaapi),
            "off" => Ok(Self::Off),
            value => bail!(
                "invalid COMPUTERUSE_VIDEO_ACCEL value {value:?}; expected auto, vaapi, or off"
            ),
        }
    }
}

#[derive(Debug)]
struct VideoAcceleration {
    mode: AccelerationMode,
    devices: Vec<PathBuf>,
    explicit_device: bool,
}

impl VideoAcceleration {
    fn from_env() -> Result<Self> {
        let mode = AccelerationMode::parse(env::var("COMPUTERUSE_VIDEO_ACCEL").ok().as_deref())?;
        let explicit = env::var_os("COMPUTERUSE_VAAPI_DEVICE").map(PathBuf::from);
        let explicit_device = explicit.is_some();
        let devices = if mode == AccelerationMode::Off {
            Vec::new()
        } else if let Some(device) = explicit {
            vec![device]
        } else {
            discover_render_nodes()?
        };
        Ok(Self {
            mode,
            devices,
            explicit_device,
        })
    }
}

/// Decode every video frame into an ordered PNG file using FFmpeg.
pub fn extract_video_frames(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
) -> Result<Vec<PathBuf>> {
    extract_video_frames_selected(input, output, FrameSelection::default())
}

/// Sample an ordered, bounded set of frames for video-driven agent observations.
pub fn extract_video_frames_sampled(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    frames_per_second: u32,
    max_frames: usize,
) -> Result<Vec<PathBuf>> {
    if frames_per_second == 0 {
        bail!("video sample rate must be greater than zero");
    }
    if max_frames == 0 {
        bail!("maximum sampled video frames must be greater than zero");
    }
    extract_video_frames_selected(
        input,
        output,
        FrameSelection {
            frames_per_second: Some(frames_per_second),
            max_frames: Some(max_frames),
        },
    )
}

fn extract_video_frames_selected(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    selection: FrameSelection,
) -> Result<Vec<PathBuf>> {
    let acceleration = VideoAcceleration::from_env()?;
    extract_video_frames_with(
        input.as_ref(),
        output.as_ref(),
        acceleration,
        selection,
        || Command::new("ffmpeg"),
    )
}

fn extract_video_frames_with<F>(
    input: &Path,
    output: &Path,
    acceleration: VideoAcceleration,
    selection: FrameSelection,
    mut ffmpeg: F,
) -> Result<Vec<PathBuf>>
where
    F: FnMut() -> Command,
{
    if !input.is_file() {
        bail!(
            "video input does not exist or is not a file: {}",
            input.display()
        );
    }
    fs::create_dir_all(output)
        .with_context(|| format!("cannot create frame output directory {}", output.display()))?;
    if !extracted_frame_paths(output)?.is_empty() {
        bail!(
            "frame output directory already contains extracted frames: {}",
            output.display()
        );
    }

    eprintln!("video acceleration mode: {:?}", acceleration.mode);
    let mut hardware_failures = Vec::new();
    if acceleration.mode != AccelerationMode::Off {
        if acceleration.devices.is_empty() {
            let message = "no accessible VA-API render nodes found under /dev/dri".to_owned();
            if acceleration.mode == AccelerationMode::Vaapi {
                bail!("VA-API decoding required but {message}");
            }
            hardware_failures.push(message);
        }

        for device in &acceleration.devices {
            if let Err(error) = validate_render_node(device) {
                let message = format!(
                    "VA-API device {} is unavailable: {error:#}",
                    device.display()
                );
                if acceleration.mode == AccelerationMode::Vaapi || acceleration.explicit_device {
                    if acceleration.mode == AccelerationMode::Vaapi {
                        bail!("{message}");
                    }
                    hardware_failures.push(message);
                    break;
                }
                hardware_failures.push(message);
                continue;
            }

            eprintln!("attempting VA-API decoding with {}", device.display());
            match decode_attempt(input, output, Some(device), selection, &mut ffmpeg) {
                Ok(paths) => {
                    eprintln!("selected VA-API device: {}", device.display());
                    return Ok(paths);
                }
                Err(error) => {
                    remove_extracted_frames(output).with_context(|| {
                        format!(
                            "cannot clean partial hardware-decoded frames in {}",
                            output.display()
                        )
                    })?;
                    hardware_failures.push(format!("{}: {error:#}", device.display()));
                    if acceleration.explicit_device {
                        break;
                    }
                }
            }
        }

        if acceleration.mode == AccelerationMode::Vaapi {
            bail!(
                "VA-API decoding failed for {}:\n{}",
                input.display(),
                hardware_failures.join("\n")
            );
        }
        eprintln!(
            "VA-API unavailable; falling back to software decoding:\n{}",
            hardware_failures.join("\n")
        );
    } else {
        eprintln!("using software video decoding");
    }

    match decode_attempt(input, output, None, selection, &mut ffmpeg) {
        Ok(paths) => Ok(paths),
        Err(software_error) => {
            remove_extracted_frames(output).with_context(|| {
                format!(
                    "cannot clean partial software-decoded frames in {}; original error: {software_error:#}",
                    output.display()
                )
            })?;
            if hardware_failures.is_empty() {
                Err(software_error)
            } else {
                Err(anyhow!(
                    "software decoding failed after VA-API attempts:\n{}\nsoftware: {software_error:#}",
                    hardware_failures.join("\n")
                ))
            }
        }
    }
}

fn decode_attempt<F>(
    input: &Path,
    output: &Path,
    device: Option<&PathBuf>,
    selection: FrameSelection,
    ffmpeg: &mut F,
) -> Result<Vec<PathBuf>>
where
    F: FnMut() -> Command,
{
    let pattern = output.join(format!("{FRAME_PREFIX}%010d{FRAME_SUFFIX}"));
    let mut command = ffmpeg();
    command.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-n"]);
    if let Some(device) = device {
        command.arg("-hwaccel").arg("vaapi");
        command.arg("-hwaccel_device").arg(device);
    }
    command.arg("-i").arg(input);
    if let Some(frames_per_second) = selection.frames_per_second {
        command.arg("-vf").arg(format!("fps={frames_per_second}"));
    } else {
        command.args(["-vsync", "0"]);
    }
    if let Some(max_frames) = selection.max_frames {
        command.arg("-frames:v").arg(max_frames.to_string());
    }
    let result = command
        .args(["-start_number", "0"])
        .arg(&pattern)
        .output()
        .context("cannot run ffmpeg; install FFmpeg and ensure `ffmpeg` is on PATH")?;
    ensure_ffmpeg_success(input, result)?;

    let paths = extracted_frame_paths(output)?;
    if paths.is_empty() {
        bail!("ffmpeg decoded no frames from {}", input.display());
    }
    Ok(paths)
}

fn ensure_ffmpeg_success(input: &Path, result: Output) -> Result<()> {
    if result.status.success() {
        return Ok(());
    }
    let diagnostic = String::from_utf8_lossy(&result.stderr);
    bail!(
        "ffmpeg could not decode video {} (status {}): {}",
        input.display(),
        result.status,
        diagnostic.trim()
    )
}

fn discover_render_nodes() -> Result<Vec<PathBuf>> {
    let directory = Path::new("/dev/dri");
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("cannot inspect VA-API devices under /dev/dri"),
    };
    let mut devices = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.strip_prefix("renderD").is_some_and(|number| {
                        number.chars().all(|character| character.is_ascii_digit())
                    })
                })
        })
        .filter(|path| fs::File::open(path).is_ok())
        .collect::<Vec<_>>();
    devices.sort();
    Ok(devices)
}

fn validate_render_node(device: &Path) -> Result<()> {
    fs::File::open(device)
        .with_context(|| format!("cannot open render node {}", device.display()))?;
    Ok(())
}

pub(crate) fn extracted_frame_paths(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = fs::read_dir(directory)
        .with_context(|| format!("cannot read frame directory {}", directory.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    let digits = name
                        .strip_prefix(FRAME_PREFIX)
                        .and_then(|name| name.strip_suffix(FRAME_SUFFIX));
                    digits.is_some_and(|digits| {
                        digits.len() == 10
                            && digits.chars().all(|character| character.is_ascii_digit())
                    })
                })
        })
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn remove_extracted_frames(directory: &Path) -> Result<()> {
    for path in extracted_frame_paths(directory)? {
        fs::remove_file(&path)
            .with_context(|| format!("cannot remove partial frame {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("computeruse-{name}-{suffix}"));
        fs::create_dir(&directory).unwrap();
        directory
    }

    fn fake_ffmpeg(directory: &Path) -> PathBuf {
        let path = directory.join("ffmpeg");
        fs::write(
            &path,
            r#"#!/bin/sh
printf '%s\n' "---" "$@" >> "$FFMPEG_LOG"
last=
hardware=false
for argument in "$@"; do
  last=$argument
  if [ "$argument" = "-hwaccel" ]; then hardware=true; fi
done
frame=$(printf '%s' "$last" | sed 's/%010d/0000000000/')
if [ "$hardware" = true ]; then
  : > "$frame"
  printf '%s\n' 'simulated unsupported profile' >&2
  exit 1
fi
: > "$frame"
if [ "${FFMPEG_SOFTWARE_FAIL:-}" = true ]; then
  printf '%s\n' 'simulated corrupt input' >&2
  exit 1
fi
exit 0
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[test]
    fn acceleration_modes_are_strictly_parsed() {
        assert_eq!(
            AccelerationMode::parse(None).unwrap(),
            AccelerationMode::Auto
        );
        assert_eq!(
            AccelerationMode::parse(Some("vaapi")).unwrap(),
            AccelerationMode::Vaapi
        );
        assert_eq!(
            AccelerationMode::parse(Some("off")).unwrap(),
            AccelerationMode::Off
        );
        assert!(AccelerationMode::parse(Some("gpu")).is_err());
    }

    #[test]
    fn extracted_paths_only_include_numbered_png_frames() {
        let directory = temporary_directory("video-paths");
        for name in [
            "frame-0000000002.png",
            "frame-0000000001.png",
            "frame-preview.png",
            "other.png",
        ] {
            fs::write(directory.join(name), []).unwrap();
        }

        let paths = extracted_frame_paths(&directory).unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths[0].ends_with("frame-0000000001.png"));
        assert!(paths[1].ends_with("frame-0000000002.png"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn auto_cleans_failed_hardware_frames_and_retries_software_once() {
        let directory = temporary_directory("video-auto");
        let output = directory.join("frames");
        let input = directory.join("input.mp4");
        let device = directory.join("renderD128");
        let log = directory.join("ffmpeg.log");
        fs::create_dir(&output).unwrap();
        fs::write(&input, []).unwrap();
        fs::write(&device, []).unwrap();
        fs::write(output.join("unrelated.txt"), "keep").unwrap();
        let executable = fake_ffmpeg(&directory);
        let acceleration = VideoAcceleration {
            mode: AccelerationMode::Auto,
            devices: vec![device.clone()],
            explicit_device: true,
        };

        let paths = extract_video_frames_with(
            &input,
            &output,
            acceleration,
            FrameSelection::default(),
            || {
                let mut command = Command::new(&executable);
                command.env("FFMPEG_LOG", &log);
                command
            },
        )
        .unwrap();

        assert_eq!(paths.len(), 1);
        assert!(output.join("unrelated.txt").is_file());
        let invocations = fs::read_to_string(log).unwrap();
        assert_eq!(invocations.matches("---").count(), 2);
        let hardware = invocations.find("-hwaccel\nvaapi").unwrap();
        let input_option = invocations.find("-i\n").unwrap();
        assert!(hardware < input_option);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn forced_vaapi_cleans_partial_frames_without_software_retry() {
        let directory = temporary_directory("video-vaapi");
        let output = directory.join("frames");
        let input = directory.join("input.mp4");
        let device = directory.join("renderD128");
        let log = directory.join("ffmpeg.log");
        fs::create_dir(&output).unwrap();
        fs::write(&input, []).unwrap();
        fs::write(&device, []).unwrap();
        let executable = fake_ffmpeg(&directory);
        let acceleration = VideoAcceleration {
            mode: AccelerationMode::Vaapi,
            devices: vec![device],
            explicit_device: true,
        };

        let error = extract_video_frames_with(
            &input,
            &output,
            acceleration,
            FrameSelection::default(),
            || {
                let mut command = Command::new(&executable);
                command.env("FFMPEG_LOG", &log);
                command
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("VA-API decoding failed"));
        assert!(extracted_frame_paths(&output).unwrap().is_empty());
        assert_eq!(fs::read_to_string(log).unwrap().matches("---").count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_software_decode_cleans_partial_frames() {
        let directory = temporary_directory("video-software-failure");
        let output = directory.join("frames");
        let input = directory.join("input.mp4");
        let log = directory.join("ffmpeg.log");
        fs::create_dir(&output).unwrap();
        fs::write(&input, []).unwrap();
        fs::write(output.join("unrelated.txt"), "keep").unwrap();
        let executable = fake_ffmpeg(&directory);
        let acceleration = VideoAcceleration {
            mode: AccelerationMode::Off,
            devices: Vec::new(),
            explicit_device: false,
        };

        let error = extract_video_frames_with(
            &input,
            &output,
            acceleration,
            FrameSelection::default(),
            || {
                let mut command = Command::new(&executable);
                command
                    .env("FFMPEG_LOG", &log)
                    .env("FFMPEG_SOFTWARE_FAIL", "true");
                command
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("simulated corrupt input"));
        assert!(extracted_frame_paths(&output).unwrap().is_empty());
        assert!(output.join("unrelated.txt").is_file());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn sampled_extraction_limits_rate_and_frame_count() {
        let directory = temporary_directory("video-sampled");
        let output = directory.join("frames");
        let input = directory.join("input.mp4");
        let log = directory.join("ffmpeg.log");
        fs::create_dir(&output).unwrap();
        fs::write(&input, []).unwrap();
        let executable = fake_ffmpeg(&directory);
        let acceleration = VideoAcceleration {
            mode: AccelerationMode::Off,
            devices: Vec::new(),
            explicit_device: false,
        };

        extract_video_frames_with(
            &input,
            &output,
            acceleration,
            FrameSelection {
                frames_per_second: Some(2),
                max_frames: Some(50),
            },
            || {
                let mut command = Command::new(&executable);
                command.env("FFMPEG_LOG", &log);
                command
            },
        )
        .unwrap();

        let invocation = fs::read_to_string(log).unwrap();
        assert!(invocation.contains("-vf\nfps=2"));
        assert!(invocation.contains("-frames:v\n50"));
        assert!(!invocation.contains("-vsync"));
        fs::remove_dir_all(directory).unwrap();
    }
}
