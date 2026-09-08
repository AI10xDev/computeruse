use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use computeruse::{Button, Controller, LinuxMouse, Listener, MouseEvent, PlaybackFilter};
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Click {
        #[arg(default_value = "left")]
        button: Button,
        #[arg(short, long, default_value_t = 1)]
        count: u32,
    },
    Press {
        #[arg(default_value = "left")]
        button: Button,
    },
    Release {
        #[arg(default_value = "left")]
        button: Button,
    },
    Move {
        dx: i32,
        dy: i32,
        #[arg(short, long, default_value_t = 0)]
        duration_ms: u64,
    },
    MoveTo {
        /// Normalized horizontal coordinate, from 0 to 65535.
        x: u16,
        /// Normalized vertical coordinate, from 0 to 65535.
        y: u16,
    },
    Wheel {
        delta: i32,
        #[arg(long)]
        horizontal: bool,
    },
    Listen,
    Record {
        output: PathBuf,
        #[arg(long, default_value = "right")]
        stop_button: Button,
    },
    Play {
        input: PathBuf,
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
        #[arg(long)]
        no_buttons: bool,
        #[arg(long)]
        no_movement: bool,
        #[arg(long)]
        no_wheel: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Listen => {
            Listener::new()?.listen(|event| println_json(&event))?;
            Ok(())
        }
        Command::Record {
            output,
            stop_button,
        } => record(output, stop_button),
        command => control(command),
    }
}

fn control(command: Command) -> Result<()> {
    let mut mouse = Controller::new(
        LinuxMouse::new().context("cannot create uinput device; check /dev/uinput permissions")?,
    );
    match command {
        Command::Click { button, count } => {
            for _ in 0..count {
                mouse.click(button)?;
            }
        }
        Command::Press { button } => mouse.press(button)?,
        Command::Release { button } => mouse.release(button)?,
        Command::Move {
            dx,
            dy,
            duration_ms: 0,
        } => mouse.move_relative(dx, dy)?,
        Command::Move {
            dx,
            dy,
            duration_ms,
        } => mouse.move_relative_over(dx, dy, Duration::from_millis(duration_ms))?,
        Command::MoveTo { x, y } => mouse.move_absolute(x, y)?,
        Command::Wheel { delta, horizontal } if horizontal => mouse.horizontal_wheel(delta)?,
        Command::Wheel { delta, .. } => mouse.wheel(delta)?,
        Command::Play {
            input,
            speed,
            no_buttons,
            no_movement,
            no_wheel,
        } => {
            if speed < 0.0 {
                bail!("speed must be zero or greater");
            }
            let reader = BufReader::new(
                File::open(&input)
                    .with_context(|| format!("cannot open recording at {}", input.display()))?,
            );
            let events: Vec<MouseEvent> = serde_json::from_reader(reader)?;
            mouse.play(
                &events,
                speed,
                PlaybackFilter {
                    buttons: !no_buttons,
                    movement: !no_movement,
                    wheel: !no_wheel,
                },
            )?;
        }
        Command::Listen | Command::Record { .. } => unreachable!(),
    }
    Ok(())
}

fn record(output: PathBuf, stop_button: Button) -> Result<()> {
    let mut listener = Listener::new()?;
    let mut events = Vec::new();
    listener.listen_until(|event| {
        let stopped = matches!(
            event,
            MouseEvent::Button {
                button,
                state: computeruse::ButtonState::Down,
                ..
            } if button == stop_button
        );
        events.push(event);
        stopped
    })?;
    let file = File::create(&output)
        .with_context(|| format!("cannot create recording at {}", output.display()))?;
    serde_json::to_writer_pretty(BufWriter::new(file), &events)?;
    Ok(())
}

fn println_json(event: &MouseEvent) {
    println!(
        "{}",
        serde_json::to_string(event).expect("event is serializable")
    );
}
