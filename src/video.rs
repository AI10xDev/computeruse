use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const FRAME_PREFIX: &str = "frame-";
const FRAME_SUFFIX: &str = ".png";

/// Decode every video frame into an ordered PNG file using FFmpeg.
pub fn extract_video_frames(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
) -> Result<Vec<PathBuf>> {
    let input = input.as_ref();
    let output = output.as_ref();
    if !input.is_file() {
        bail!(
            "video input does not exist or is not a file: {}",
            input.display()
        );
    }
    fs::create_dir_all(output)
        .with_context(|| format!("cannot create frame output directory {}", output.display()))?;

    let existing = extracted_frame_paths(output)?;
    if !existing.is_empty() {
        bail!(
            "frame output directory already contains extracted frames: {}",
            output.display()
        );
    }

    let pattern = output.join(format!("{FRAME_PREFIX}%010d{FRAME_SUFFIX}"));
    let status = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(input)
        .arg("-vsync")
        .arg("0")
        .arg("-start_number")
        .arg("0")
        .arg(&pattern)
        .status()
        .context("cannot run ffmpeg; install FFmpeg and ensure `ffmpeg` is on PATH")?;
    if !status.success() {
        remove_extracted_frames(output);
        bail!("ffmpeg could not decode video {}", input.display());
    }

    let paths = extracted_frame_paths(output)?;
    if paths.is_empty() {
        bail!("ffmpeg decoded no frames from {}", input.display());
    }
    Ok(paths)
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

fn remove_extracted_frames(directory: &Path) {
    if let Ok(paths) = extracted_frame_paths(directory) {
        for path in paths {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn extracted_paths_only_include_numbered_png_frames() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("computeruse-video-{suffix}"));
        fs::create_dir(&directory).unwrap();
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
}
