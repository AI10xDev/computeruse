use anyhow::Result;
use clap::Parser;
use computeruse::extract_video_frames;
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Splice a video into ordered PNG frames")]
struct Cli {
    /// Video file to decode, such as an MP4 file.
    #[arg(long)]
    frame: PathBuf,
    /// Directory in which to write frame-0000000000.png and later frames.
    #[arg(short, long)]
    output: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let frames = extract_video_frames(cli.frame, &cli.output)?;
    println!("wrote {} frames to {}", frames.len(), cli.output.display());
    Ok(())
}
