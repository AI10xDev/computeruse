use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use computeruse::{
    Action, Button, Controller, FrameSequence, GptAstraPolicy, Key, KeyState, KeyboardController,
    KeyboardEvent, KeyboardListener, KeyboardPlaybackFilter, LinuxKeyboard, LinuxMouse, Listener,
    MouseEvent, MouseTrajectory, PlaybackFilter, Point, Policy, Transition, X11Cursor,
    extract_video_frames,
};
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ESCAPE_SCAN_CODE: u16 = 1;
const HOME_X: i16 = 960;
const HOME_Y: i16 = 540;
const HOME_SCREEN: usize = 0;

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
    /// Record mouse events until Escape is pressed.
    Record {
        output: PathBuf,
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
    /// Run the instruction-driven visual policy over each frame of a video.
    Agent {
        /// Video file to process, such as an MP4 file.
        #[arg(long)]
        frame: PathBuf,
        /// Keep extracted PNG frames in this directory instead of a temporary directory.
        #[arg(long)]
        frame_output: Option<PathBuf>,
        #[arg(long)]
        instructions: PathBuf,
        #[arg(long, default_value = "gpt-Astra")]
        model: String,
        #[arg(long, default_value_t = 3)]
        trajectory: usize,
        #[arg(long, default_value_t = 50)]
        max_steps: usize,
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
        Command::Record { output } => record(output),
        Command::KeyboardListen => {
            KeyboardListener::new()?.listen(|event| println_json(&event))?;
            Ok(())
        }
        Command::KeyboardRecord { output, stop_key } => keyboard_record(output, stop_key),
        Command::Agent {
            frame,
            frame_output,
            instructions,
            model,
            trajectory,
            max_steps,
            execute,
            trace,
        } => run_agent(AgentOptions {
            frame,
            frame_output,
            instructions,
            model,
            trajectory,
            max_steps,
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
            calibrate_cursor("playback")?;
            MouseTrajectory::from_events(home_point(), &events)
                .context("recording contains an invalid mouse trajectory")?;
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
    frame: PathBuf,
    frame_output: Option<PathBuf>,
    instructions: PathBuf,
    model: String,
    trajectory: usize,
    max_steps: usize,
    execute: bool,
    trace: Option<PathBuf>,
}

struct TemporaryFrameDirectory {
    path: PathBuf,
}

impl TemporaryFrameDirectory {
    fn new() -> Result<Self> {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before the Unix epoch")?
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "computeruse-video-frames-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir(&path)
            .with_context(|| format!("cannot create temporary directory {}", path.display()))?;
        Ok(Self { path })
    }
}

impl Drop for TemporaryFrameDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run_agent(options: AgentOptions) -> Result<()> {
    let instructions = fs::read_to_string(&options.instructions).with_context(|| {
        format!(
            "cannot read GUI instructions at {}",
            options.instructions.display()
        )
    })?;
    let temporary_frames = options
        .frame_output
        .is_none()
        .then(TemporaryFrameDirectory::new)
        .transpose()?;
    let frame_output = options
        .frame_output
        .as_deref()
        .or_else(|| {
            temporary_frames
                .as_ref()
                .map(|directory| directory.path.as_path())
        })
        .expect("a persistent or temporary frame output exists");
    let paths = extract_video_frames(&options.frame, frame_output)?;
    let mut frames = FrameSequence::new(paths, options.trajectory)?;
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
        let trajectory = frames.advance()?.with_context(|| {
            format!("video ended before the agent completed the instructions at step {step}")
        })?;
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

fn record(output: PathBuf) -> Result<()> {
    let mut listener = Listener::new()?;
    let mut keyboard = KeyboardListener::new()?;
    calibrate_cursor("recording")?;

    let mut events = Vec::new();
    let (stop_sender, stop_receiver) = mpsc::sync_channel(1);
    let keyboard_thread = thread::spawn(move || {
        let result = keyboard.listen_until(|event| is_escape_press(&event));
        let _ = stop_sender.send(result);
    });
    let mut keyboard_result = None;
    listener.listen_until_cancelled(
        |event| {
            events.push(event);
            false
        },
        || match stop_receiver.try_recv() {
            Ok(result) => {
                keyboard_result = Some(result);
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => true,
        },
    )?;
    keyboard_thread
        .join()
        .map_err(|_| anyhow::anyhow!("keyboard listener thread panicked"))?;
    keyboard_result
        .context("keyboard listener stopped unexpectedly")?
        .context("cannot listen for Escape while recording")?;
    MouseTrajectory::from_events(home_point(), &events)
        .context("recorded mouse trajectory exceeds the coordinate range")?;
    let file = File::create(&output)
        .with_context(|| format!("cannot create recording at {}", output.display()))?;
    serde_json::to_writer_pretty(BufWriter::new(file), &events)?;
    Ok(())
}

fn home_point() -> Point {
    Point {
        x: i32::from(HOME_X),
        y: i32::from(HOME_Y),
    }
}

fn calibrate_cursor(operation: &str) -> Result<()> {
    let cursor = X11Cursor::connect(HOME_SCREEN)
        .with_context(|| format!("cannot initialize X11 cursor for {operation}"))?;
    let initial = cursor
        .location()
        .with_context(|| format!("cannot query cursor before {operation}"))?;
    let calibrated = cursor
        .home(HOME_X, HOME_Y)
        .with_context(|| format!("cannot calibrate cursor before {operation}"))?;
    eprintln!("cursor before {operation}: {initial}");
    eprintln!("cursor calibrated for {operation}: {calibrated}");
    Ok(())
}

fn is_escape_press(event: &KeyboardEvent) -> bool {
    event.key.scan_code == ESCAPE_SCAN_CODE && event.state == KeyState::Down && !event.repeat
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

    #[test]
    fn agent_accepts_a_video_with_the_frame_flag() {
        let cli = Cli::try_parse_from([
            "computeruse",
            "agent",
            "--frame",
            "recording.mp4",
            "--instructions",
            "steps.txt",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Agent { frame, .. } if frame == *"recording.mp4"
        ));
    }

    #[test]
    fn record_only_requires_an_output_path() {
        let cli = Cli::try_parse_from(["computeruse", "record", "session.json"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Record { output } if output == *"session.json"
        ));
    }

    #[test]
    fn escape_press_stops_mouse_recording() {
        let mut event = KeyboardEvent {
            key: Key::from_scan_code(ESCAPE_SCAN_CODE),
            state: KeyState::Down,
            repeat: false,
            time: 0.0,
        };
        assert!(is_escape_press(&event));

        event.repeat = true;
        assert!(!is_escape_press(&event));
        event.repeat = false;
        event.state = KeyState::Up;
        assert!(!is_escape_press(&event));
    }
}
