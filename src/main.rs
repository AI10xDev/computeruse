use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use computeruse::{
    Action, Button, Controller, FrameSource, GptAstraPolicy, Key, KeyState, KeyboardController,
    KeyboardEvent, KeyboardListener, KeyboardPlaybackFilter, LinuxKeyboard, LinuxMouse, Listener,
    MouseEvent, PlaybackFilter, Policy, Transition,
};
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::thread;
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
    KeyPress {
        key: Key,
    },
    KeyRelease {
        key: Key,
    },
    Hotkey {
        /// Simultaneous keys, for example `ctrl+shift+a`.
        keys: String,
    },
    KeyboardListen,
    KeyboardRecord {
        output: PathBuf,
        #[arg(long, default_value = "escape")]
        stop_key: Key,
    },
    KeyboardPlay {
        input: PathBuf,
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
        #[arg(long)]
        no_press: bool,
        #[arg(long)]
        no_release: bool,
    },
    /// Run the instruction-driven visual policy over screencast frames.
    Agent {
        #[arg(long)]
        frames: PathBuf,
        #[arg(long)]
        instructions: PathBuf,
        #[arg(long, default_value = "gpt-Astra")]
        model: String,
        #[arg(long, default_value_t = 3)]
        trajectory: usize,
        #[arg(long, default_value_t = 50)]
        max_steps: usize,
        #[arg(long, default_value_t = 10_000)]
        frame_timeout_ms: u64,
        /// Actually inject policy actions. Without this flag the agent is a dry run.
        #[arg(long)]
        execute: bool,
        /// Optional JSONL transition log for evaluation or offline learning.
        #[arg(long)]
        trace: Option<PathBuf>,
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
        Command::KeyboardListen => {
            KeyboardListener::new()?.listen(|event| println_json(&event))?;
            Ok(())
        }
        Command::KeyboardRecord { output, stop_key } => keyboard_record(output, stop_key),
        Command::Agent {
            frames,
            instructions,
            model,
            trajectory,
            max_steps,
            frame_timeout_ms,
            execute,
            trace,
        } => run_agent(AgentOptions {
            frames,
            instructions,
            model,
            trajectory,
            max_steps,
            frame_timeout: Duration::from_millis(frame_timeout_ms),
            execute,
            trace,
        }),
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
            mouse
                .home()
                .context("cannot move mouse to the home position before playback")?;
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
        Command::KeyPress { key } => KeyboardController::new(
            LinuxKeyboard::new()
                .context("cannot create virtual keyboard; check /dev/uinput permissions")?,
        )
        .press(&key)?,
        Command::KeyRelease { key } => KeyboardController::new(
            LinuxKeyboard::new()
                .context("cannot create virtual keyboard; check /dev/uinput permissions")?,
        )
        .release(&key)?,
        Command::Hotkey { keys } => KeyboardController::new(
            LinuxKeyboard::new()
                .context("cannot create virtual keyboard; check /dev/uinput permissions")?,
        )
        .send_hotkey(&keys)
        .map_err(anyhow::Error::msg)?,
        Command::KeyboardPlay {
            input,
            speed,
            no_press,
            no_release,
        } => {
            if speed < 0.0 {
                bail!("speed must be zero or greater");
            }
            let reader = BufReader::new(
                File::open(&input)
                    .with_context(|| format!("cannot open recording at {}", input.display()))?,
            );
            let events: Vec<KeyboardEvent> = serde_json::from_reader(reader)?;
            KeyboardController::new(
                LinuxKeyboard::new()
                    .context("cannot create virtual keyboard; check /dev/uinput permissions")?,
            )
            .play(
                &events,
                speed,
                KeyboardPlaybackFilter {
                    press: !no_press,
                    release: !no_release,
                },
            )?;
        }
        Command::Listen
        | Command::Record { .. }
        | Command::KeyboardListen
        | Command::KeyboardRecord { .. }
        | Command::Agent { .. } => unreachable!(),
    }
    Ok(())
}

fn keyboard_record(output: PathBuf, stop_key: Key) -> Result<()> {
    let mut listener = KeyboardListener::new()?;
    let mut events = Vec::new();
    let mut stop_pressed = false;
    listener.listen_until(|event| {
        if event.key.scan_code == stop_key.scan_code
            && event.state == KeyState::Down
            && !event.repeat
        {
            stop_pressed = true;
        }
        let stopped = stop_pressed
            && event.key.scan_code == stop_key.scan_code
            && event.state == KeyState::Up;
        events.push(event);
        stopped
    })?;
    events.sort_by(|left, right| left.time.total_cmp(&right.time));
    let file = File::create(&output)
        .with_context(|| format!("cannot create recording at {}", output.display()))?;
    serde_json::to_writer_pretty(BufWriter::new(file), &events)?;
    Ok(())
}

struct AgentOptions {
    frames: PathBuf,
    instructions: PathBuf,
    model: String,
    trajectory: usize,
    max_steps: usize,
    frame_timeout: Duration,
    execute: bool,
    trace: Option<PathBuf>,
}

fn run_agent(options: AgentOptions) -> Result<()> {
    let instructions = fs::read_to_string(&options.instructions).with_context(|| {
        format!(
            "cannot read GUI instructions at {}",
            options.instructions.display()
        )
    })?;
    let mut frames = FrameSource::new(options.frames, options.trajectory)?;
    let policy = GptAstraPolicy::from_env(options.model)?;
    let mut transitions = Vec::new();
    let mut previous_progress = 0.0;
    let mut devices = if options.execute {
        Some((
            Controller::new(
                LinuxMouse::new()
                    .context("cannot create virtual mouse; check /dev/uinput permissions")?,
            ),
            KeyboardController::new(
                LinuxKeyboard::new()
                    .context("cannot create virtual keyboard; check /dev/uinput permissions")?,
            ),
        ))
    } else {
        None
    };

    for step in 0..options.max_steps {
        let trajectory = frames
            .next(options.frame_timeout)?
            .with_context(|| format!("timed out waiting for a new frame at step {step}"))?;
        let frame = trajectory
            .back()
            .expect("trajectory has a new frame")
            .path
            .clone();
        let decision = policy.decide(&instructions, trajectory, &transitions)?;
        if decision.actions.len() > 16 {
            bail!("policy proposed more than 16 actions in one step");
        }
        validate_actions(&decision.actions)?;
        if let Some((mouse, keyboard)) = &mut devices {
            execute_actions(mouse, keyboard, &decision.actions)?;
        }
        let progress = decision.progress.clamp(0.0, 1.0);
        let transition = Transition {
            step,
            frame,
            actions: decision.actions,
            progress,
            reward: progress - previous_progress,
            completed: decision.completed,
            executed: options.execute,
        };
        previous_progress = progress;
        println_json(&transition);
        if let Some(path) = &options.trace {
            append_json_line(path, &transition)?;
        }
        let completed = transition.completed;
        transitions.push(transition);
        if completed {
            return Ok(());
        }
    }
    bail!("agent reached max steps without completing the instructions")
}

fn execute_actions(
    mouse: &mut Controller<LinuxMouse>,
    keyboard: &mut KeyboardController<LinuxKeyboard>,
    actions: &[Action],
) -> Result<()> {
    for action in actions {
        match action {
            Action::MouseMove { dx, dy } => mouse.move_relative(*dx, *dy)?,
            Action::MouseMoveTo { x, y } => mouse.move_absolute(*x, *y)?,
            Action::MouseClick { button } => mouse.click(*button)?,
            Action::Scroll { delta, horizontal } if *horizontal => {
                mouse.horizontal_wheel(*delta)?
            }
            Action::Scroll { delta, .. } => mouse.wheel(*delta)?,
            Action::KeyTap { key } => keyboard.send_hotkey(key).map_err(anyhow::Error::msg)?,
            Action::Hotkey { keys } => keyboard.send_hotkey(keys).map_err(anyhow::Error::msg)?,
            Action::Wait { milliseconds } => {
                if *milliseconds > 30_000 {
                    bail!("policy wait action exceeds 30 seconds");
                }
                thread::sleep(Duration::from_millis(*milliseconds));
            }
        }
    }
    Ok(())
}

fn validate_actions(actions: &[Action]) -> Result<()> {
    for action in actions {
        match action {
            Action::MouseMove { dx, dy }
                if dx.unsigned_abs() > 32_767 || dy.unsigned_abs() > 32_767 =>
            {
                bail!("policy relative mouse movement exceeds +/-32767")
            }
            Action::Scroll { delta, .. } if delta.unsigned_abs() > 100 => {
                bail!("policy scroll action exceeds +/-100")
            }
            Action::KeyTap { key } => {
                validate_policy_key(key)?;
            }
            Action::Hotkey { keys } => {
                let parsed = keys
                    .split('+')
                    .map(str::parse::<Key>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(anyhow::Error::msg)?;
                if parsed.is_empty() {
                    bail!("policy hotkey cannot be empty");
                }
                if let Some(key) = parsed
                    .iter()
                    .find(|key| !LinuxKeyboard::supports_scan_code(key.scan_code))
                {
                    bail!("policy key is not injectable on Linux: {key}");
                }
            }
            Action::Wait { milliseconds } if *milliseconds > 30_000 => {
                bail!("policy wait action exceeds 30 seconds")
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_policy_key(value: &str) -> Result<Key> {
    let key = value.parse::<Key>().map_err(anyhow::Error::msg)?;
    if !LinuxKeyboard::supports_scan_code(key.scan_code) {
        bail!("policy key is not injectable on Linux: {key}");
    }
    Ok(key)
}

fn append_json_line(path: &PathBuf, value: &impl Serialize) -> Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn record(output: PathBuf, stop_button: Button) -> Result<()> {
    // Open physical devices before creating uinput devices so recording cannot
    // include events from the virtual mouse used for homing.
    let mut listener = Listener::new()?;
    let mut mouse = Controller::new(
        LinuxMouse::new().context("cannot create uinput device; check /dev/uinput permissions")?,
    );
    mouse
        .home()
        .context("cannot move mouse to the home position before recording")?;

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

fn println_json(event: &impl Serialize) {
    println!(
        "{}",
        serde_json::to_string(event).expect("event is serializable")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_actions_are_validated_before_execution() {
        assert!(validate_actions(&[Action::MouseMove { dx: 32_768, dy: 0 }]).is_err());
        assert!(
            validate_actions(&[Action::Wait {
                milliseconds: 30_001,
            }])
            .is_err()
        );
        assert!(
            validate_actions(&[Action::KeyTap {
                key: "code:65535".into(),
            }])
            .is_err()
        );
        assert!(
            validate_actions(&[
                Action::MouseMove { dx: -32_767, dy: 1 },
                Action::Scroll {
                    delta: 100,
                    horizontal: false,
                },
                Action::Hotkey {
                    keys: "ctrl+a".into(),
                },
            ])
            .is_ok()
        );
    }
}
