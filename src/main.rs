use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use computeruse::{
    Action, Button, Controller, CursorBackend, CursorMotion, Frame, FrameSequence, FrameSource,
    GptAstraPolicy, Key, KeyState, KeyboardBackend, KeyboardController, KeyboardEvent,
    KeyboardListener, KeyboardPlaybackFilter, LinuxKeyboard, LinuxMouse, Listener, MouseBackend,
    MouseEvent, MouseTrajectory, PlaybackFilter, Point, Policy, Transition, X11ButtonListener,
    X11Cursor, extract_video_frames_sampled, move_cursor_to,
};
use serde::Serialize;
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ESCAPE_SCAN_CODE: u16 = 1;
const HOME_X: i16 = 960;
const HOME_Y: i16 = 540;
const HOME_SCREEN: usize = 0;
const AGENT_VIDEO_FRAMES_PER_SECOND: u32 = 2;

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
    /// Run the instruction-driven visual policy over live frames or a video.
    Agent {
        /// Directory to watch for live PNG, JPEG, or WebP frames.
        #[arg(long, conflicts_with = "frame", required_unless_present = "frame")]
        frames: Option<PathBuf>,
        /// Video file to process, such as an MP4 file.
        #[arg(long, conflicts_with = "frames", required_unless_present = "frames")]
        frame: Option<PathBuf>,
        /// Keep extracted PNG frames in this directory instead of a temporary directory.
        #[arg(long, requires = "frame")]
        frame_output: Option<PathBuf>,
        #[arg(long)]
        instructions: PathBuf,
        #[arg(long, default_value = "gpt-6-astra")]
        model: String,
        #[arg(long, default_value_t = 3)]
        trajectory: usize,
        #[arg(long, default_value_t = 50)]
        max_steps: usize,
        /// Maximum time to wait for the next live frame.
        #[arg(long, default_value_t = 10_000)]
        frame_timeout_ms: u64,
        /// Actually inject policy actions. Without this flag the agent is a dry run.
        #[arg(long)]
        execute: bool,
        /// Proportional pixel control requires native X11; direct uses unverified uinput moves.
        #[arg(long, value_enum, default_value = "proportional")]
        mouse_control: MouseControl,
        /// Optional JSONL transition log for evaluation or offline learning.
        #[arg(long)]
        trace: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum MouseControl {
    Proportional,
    Direct,
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
            frames,
            frame,
            frame_output,
            instructions,
            model,
            trajectory,
            max_steps,
            frame_timeout_ms,
            execute,
            mouse_control,
            trace,
        } => run_agent(AgentOptions {
            frames,
            frame,
            frame_output,
            instructions,
            model,
            trajectory,
            max_steps,
            frame_timeout: Duration::from_millis(frame_timeout_ms),
            execute,
            mouse_control,
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
    frames: Option<PathBuf>,
    frame: Option<PathBuf>,
    frame_output: Option<PathBuf>,
    instructions: PathBuf,
    model: String,
    trajectory: usize,
    max_steps: usize,
    frame_timeout: Duration,
    execute: bool,
    mouse_control: MouseControl,
    trace: Option<PathBuf>,
}

enum AgentFrameSource {
    Live(FrameSource),
    Video(FrameSequence),
}

impl AgentFrameSource {
    fn is_live(&self) -> bool {
        matches!(self, Self::Live(_))
    }

    fn next(&mut self, timeout: Duration) -> Result<Option<&VecDeque<Frame>>> {
        match self {
            Self::Live(source) => source.next(timeout),
            Self::Video(source) => source.advance(),
        }
    }

    fn discard_live_frames(&mut self) -> Result<()> {
        if let Self::Live(source) = self {
            source.discard_existing()?;
        }
        Ok(())
    }
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
    let temporary_frames = (options.frame.is_some() && options.frame_output.is_none())
        .then(TemporaryFrameDirectory::new)
        .transpose()?;
    let mut frames = if let Some(directory) = options.frames {
        AgentFrameSource::Live(FrameSource::new(directory, options.trajectory)?)
    } else {
        let video = options
            .frame
            .as_ref()
            .expect("clap requires a frame source");
        let frame_output = options
            .frame_output
            .as_deref()
            .or_else(|| {
                temporary_frames
                    .as_ref()
                    .map(|directory| directory.path.as_path())
            })
            .expect("a video frame output exists");
        let paths = extract_video_frames_sampled(
            video,
            frame_output,
            AGENT_VIDEO_FRAMES_PER_SECOND,
            options.max_steps,
        )?;
        AgentFrameSource::Video(FrameSequence::new(paths, options.trajectory)?)
    };
    let policy = GptAstraPolicy::from_env(options.model)?;
    let mut transitions = Vec::new();
    let mut previous_progress = 0.0;
    let mut cursor = if options.execute && options.mouse_control == MouseControl::Proportional {
        let mut cursor = X11Cursor::connect_default().context(
            "cannot initialize proportional cursor control; use native X11 or explicitly select --mouse-control direct for unverified movement",
        )?;
        cursor.require_native_x11()?;
        let (width, height) = cursor.dimensions()?;
        cursor.position()?;
        eprintln!("agent mouse control: proportional, {width}x{height} X11 desktop pixels");
        Some(cursor)
    } else {
        if options.execute {
            eprintln!(
                "agent mouse control: direct; cursor arrival and trajectory are not verified"
            );
        }
        None
    };
    if options.execute && !frames.is_live() {
        eprintln!(
            "warning: prerecorded video cannot show the effects of injected actions; use --frames for live GUI feedback"
        );
    }
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
        let missing_frame_context = if frames.is_live() {
            format!(
                "timed out after {} ms waiting for a new live frame at step {step}",
                options.frame_timeout.as_millis()
            )
        } else {
            format!("video ended before the agent completed the instructions at step {step}")
        };
        let trajectory = frames
            .next(options.frame_timeout)?
            .with_context(|| missing_frame_context)?;
        let frame = trajectory
            .back()
            .expect("trajectory has a new frame")
            .path
            .clone();
        let policy_started = Instant::now();
        let decision = policy.decide(&instructions, trajectory, &transitions)?;
        let policy_elapsed = policy_started.elapsed();
        let (input_elapsed, cursor_motion) = if let Some((mouse, keyboard)) = &mut devices {
            let input_started = Instant::now();
            let motion = execute_actions(
                mouse,
                keyboard,
                cursor
                    .as_mut()
                    .map(|cursor| cursor as &mut dyn CursorBackend),
                &decision.actions,
            )?;
            let elapsed = input_started.elapsed();
            frames.discard_live_frames()?;
            (elapsed, motion)
        } else {
            validate_actions(&decision.actions)?;
            (Duration::ZERO, Vec::new())
        };
        eprintln!(
            "agent step {step}: policy={} ms, input={:.3} ms{}",
            policy_elapsed.as_millis(),
            input_elapsed.as_secs_f64() * 1000.0,
            if options.execute { "" } else { " (dry run)" }
        );
        let progress = decision.progress.clamp(0.0, 1.0);
        let transition = Transition {
            step,
            frame,
            actions: decision.actions,
            progress,
            reward: progress - previous_progress,
            completed: decision.completed,
            executed: options.execute,
            cursor_motion,
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

fn execute_actions<M: MouseBackend, K: KeyboardBackend>(
    mouse: &mut Controller<M>,
    keyboard: &mut KeyboardController<K>,
    mut cursor: Option<&mut dyn CursorBackend>,
    actions: &[Action],
) -> Result<Vec<CursorMotion>>
where
    M::Error: std::error::Error + Send + Sync + 'static,
    K::Error: std::fmt::Display,
{
    validate_actions(actions)?;
    let mut cursor_motion = Vec::new();
    for action in actions {
        match action {
            Action::MouseMove { dx, dy } => mouse.move_relative(*dx, *dy)?,
            Action::MouseMoveTo { x, y } => {
                if let Some(cursor) = cursor.as_deref_mut() {
                    cursor_motion.push(move_cursor_to(cursor, *x, *y)?);
                } else {
                    mouse.move_absolute(*x, *y)?;
                }
            }
            Action::MouseClick { button } => mouse.click(*button)?,
            Action::Scroll { delta, horizontal } if *horizontal => {
                mouse.horizontal_wheel(*delta)?
            }
            Action::Scroll { delta, .. } => mouse.wheel(*delta)?,
            Action::KeyTap { key } => keyboard.send_hotkey(key).map_err(anyhow::Error::msg)?,
            Action::Hotkey { keys } => keyboard.send_hotkey(keys).map_err(anyhow::Error::msg)?,
            Action::KeySequence { keys } => {
                for key in keys {
                    keyboard.send_hotkey(key).map_err(anyhow::Error::msg)?;
                }
            }
            Action::Wait { milliseconds } => {
                thread::sleep(Duration::from_millis(*milliseconds));
            }
        }
    }
    Ok(cursor_motion)
}

fn validate_actions(actions: &[Action]) -> Result<()> {
    if actions.len() > 16 {
        bail!("policy proposed more than 16 actions in one step");
    }
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
                validate_policy_hotkey(keys)?;
            }
            Action::KeySequence { keys } => {
                if keys.is_empty() || keys.len() > 256 {
                    bail!("policy key sequence must contain 1..256 keys");
                }
                for key in keys {
                    validate_policy_hotkey(key)?;
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

fn validate_policy_hotkey(value: &str) -> Result<()> {
    for key in value.split('+') {
        validate_policy_key(key)?;
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
    let button_listener = X11ButtonListener::connect(HOME_SCREEN)
        .context("cannot initialize XInput button recording")?;
    calibrate_cursor("recording")?;

    let mut events = Vec::new();
    let stop_buttons = Arc::new(AtomicBool::new(false));
    let button_stop = Arc::clone(&stop_buttons);
    let button_thread = thread::spawn(move || {
        let mut events = Vec::new();
        let result = button_listener.listen_until_cancelled(
            |event| {
                events.push(event);
                false
            },
            || button_stop.load(Ordering::Acquire),
        );
        (events, result)
    });
    let (stop_sender, stop_receiver) = mpsc::sync_channel(1);
    let keyboard_thread = thread::spawn(move || {
        let result = keyboard.listen_until(|event| is_escape_press(&event));
        let _ = stop_sender.send(result);
    });
    let mut keyboard_result = None;
    let listener_result = listener.listen_until_cancelled(
        |event| {
            if !matches!(event, MouseEvent::Button { .. }) {
                events.push(event);
            }
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
    );
    stop_buttons.store(true, Ordering::Release);
    keyboard_thread
        .join()
        .map_err(|_| anyhow::anyhow!("keyboard listener thread panicked"))?;
    keyboard_result
        .context("keyboard listener stopped unexpectedly")?
        .context("cannot listen for Escape while recording")?;
    let (button_events, button_result) = button_thread
        .join()
        .map_err(|_| anyhow::anyhow!("XInput button listener thread panicked"))?;
    listener_result.context("cannot record mouse movement")?;
    button_result.context("cannot record XInput button events")?;
    events.extend(button_events);
    events.sort_by(|left, right| left.timestamp().total_cmp(&right.timestamp()));
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
    use computeruse::ButtonState;
    use std::convert::Infallible;

    struct NoMouse;

    impl MouseBackend for NoMouse {
        type Error = Infallible;

        fn button(&mut self, _: Button, _: ButtonState) -> Result<(), Self::Error> {
            unreachable!("keyboard actions must not emit mouse events")
        }

        fn move_relative(&mut self, _: i32, _: i32) -> Result<(), Self::Error> {
            unreachable!("keyboard actions must not emit mouse events")
        }

        fn move_absolute(&mut self, _: u16, _: u16) -> Result<(), Self::Error> {
            unreachable!("keyboard actions must not emit mouse events")
        }

        fn wheel(&mut self, _: i32, _: bool) -> Result<(), Self::Error> {
            unreachable!("keyboard actions must not emit mouse events")
        }
    }

    struct RecordingKeyboard<'a>(&'a mut Vec<(u16, KeyState)>);

    impl KeyboardBackend for RecordingKeyboard<'_> {
        type Error = Infallible;

        fn key(&mut self, scan_code: u16, state: KeyState) -> Result<(), Self::Error> {
            self.0.push((scan_code, state));
            Ok(())
        }
    }

    struct RecordingMouse<'a>(&'a mut Vec<String>);

    impl MouseBackend for RecordingMouse<'_> {
        type Error = Infallible;

        fn button(&mut self, button: Button, state: ButtonState) -> Result<(), Self::Error> {
            self.0.push(format!("{button}:{state:?}"));
            Ok(())
        }

        fn move_relative(&mut self, dx: i32, dy: i32) -> Result<(), Self::Error> {
            self.0.push(format!("relative:{dx},{dy}"));
            Ok(())
        }

        fn move_absolute(&mut self, x: u16, y: u16) -> Result<(), Self::Error> {
            self.0.push(format!("absolute:{x},{y}"));
            Ok(())
        }

        fn wheel(&mut self, _: i32, _: bool) -> Result<(), Self::Error> {
            unreachable!("these actions must not scroll")
        }
    }

    struct ObservedCursor {
        reads: usize,
        fail_after: usize,
    }

    impl CursorBackend for ObservedCursor {
        fn dimensions(&mut self) -> Result<(u16, u16)> {
            Ok((1920, 1080))
        }

        fn position(&mut self) -> Result<Point> {
            self.reads += 1;
            if self.reads > self.fail_after {
                bail!("cursor feedback lost");
            }
            Ok(Point { x: 960, y: 540 })
        }

        fn move_to(&mut self, _: Point) -> Result<()> {
            unreachable!("cursor is already at the requested target")
        }
    }

    #[test]
    fn targeted_move_verifies_arrival_and_logs_feedback_before_clicking() {
        let mut mouse_events = Vec::new();
        let mut key_events = Vec::new();
        let mut mouse = Controller::new(RecordingMouse(&mut mouse_events));
        let mut keyboard = KeyboardController::new(RecordingKeyboard(&mut key_events));
        let mut cursor = ObservedCursor {
            reads: 0,
            fail_after: 2,
        };
        let motion = execute_actions(
            &mut mouse,
            &mut keyboard,
            Some(&mut cursor),
            &[
                Action::MouseMoveTo {
                    x: 32_768,
                    y: 32_768,
                },
                Action::MouseClick {
                    button: Button::Left,
                },
            ],
        )
        .unwrap();

        assert_eq!(cursor.reads, 2);
        assert_eq!(mouse_events, ["left:Down", "left:Up"]);
        assert!(key_events.is_empty());
        assert_eq!(motion.len(), 1);
        assert_eq!(motion[0].estimate.position, Point { x: 960, y: 540 });
        assert_eq!(motion[0].residual_pixels, 0.0);
        let trace = serde_json::to_value(Transition {
            step: 0,
            frame: "frame.png".into(),
            actions: vec![],
            progress: 0.0,
            reward: 0.0,
            completed: false,
            executed: true,
            cursor_motion: motion,
        })
        .unwrap();
        assert_eq!(trace["cursor_motion"][0]["target"]["x"], 960);
        assert_eq!(
            trace["cursor_motion"][0]["estimate"]["velocity"],
            serde_json::json!([0.0, 0.0])
        );
    }

    #[test]
    fn lost_arrival_feedback_prevents_following_click_and_typing() {
        let mut events = Vec::new();
        let mut mouse = Controller::new(NoMouse);
        let mut keyboard = KeyboardController::new(RecordingKeyboard(&mut events));
        let mut cursor = ObservedCursor {
            reads: 0,
            fail_after: 1,
        };
        let error = execute_actions(
            &mut mouse,
            &mut keyboard,
            Some(&mut cursor),
            &[
                Action::MouseMoveTo {
                    x: 32_768,
                    y: 32_768,
                },
                Action::MouseClick {
                    button: Button::Left,
                },
                Action::KeyTap { key: "a".into() },
            ],
        )
        .unwrap_err();

        assert!(error.to_string().contains("cursor feedback lost"));
        assert_eq!(cursor.reads, 2);
        assert!(events.is_empty());
    }

    #[test]
    fn direct_absolute_and_raw_relative_movement_keep_their_units() {
        let mut mouse_events = Vec::new();
        let mut key_events = Vec::new();
        let mut mouse = Controller::new(RecordingMouse(&mut mouse_events));
        let mut keyboard = KeyboardController::new(RecordingKeyboard(&mut key_events));
        let mut cursor = ObservedCursor {
            reads: 0,
            fail_after: 0,
        };
        assert!(
            execute_actions(
                &mut mouse,
                &mut keyboard,
                None,
                &[Action::MouseMoveTo { x: 1234, y: 56_789 }],
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            execute_actions(
                &mut mouse,
                &mut keyboard,
                Some(&mut cursor),
                &[Action::MouseMove { dx: -7, dy: 3 }],
            )
            .unwrap()
            .is_empty()
        );
        assert_eq!(mouse_events, ["absolute:1234,56789", "relative:-7,3"]);
        assert_eq!(cursor.reads, 0);
    }

    #[test]
    fn invalid_batch_does_not_start_cursor_control() {
        let mut events = Vec::new();
        let mut mouse = Controller::new(NoMouse);
        let mut keyboard = KeyboardController::new(RecordingKeyboard(&mut events));
        let mut cursor = ObservedCursor {
            reads: 0,
            fail_after: 0,
        };
        assert!(
            execute_actions(
                &mut mouse,
                &mut keyboard,
                Some(&mut cursor),
                &[
                    Action::MouseMoveTo {
                        x: 32_768,
                        y: 32_768
                    },
                    Action::KeyTap {
                        key: "unknown".into()
                    },
                ],
            )
            .is_err()
        );
        assert_eq!(cursor.reads, 0);
        assert!(events.is_empty());
    }

    #[test]
    fn key_sequence_emits_each_tap_once_and_preserves_double_letters() {
        let mut events = Vec::new();
        let mut mouse = Controller::new(NoMouse);
        let mut keyboard = KeyboardController::new(RecordingKeyboard(&mut events));
        let mut keys: Vec<String> = "hello.realestate.com.au"
            .chars()
            .map(|key| key.to_string())
            .collect();
        keys.push("shift+a".into());
        keys.push("enter".into());

        execute_actions(
            &mut mouse,
            &mut keyboard,
            None,
            &[Action::KeySequence { keys }],
        )
        .unwrap();

        let mut expected: Vec<_> = [
            35, 18, 38, 38, 24, 52, 19, 18, 30, 38, 18, 31, 20, 30, 20, 18, 52, 46, 24, 50, 52, 30,
            22,
        ]
        .into_iter()
        .flat_map(|code| [(code, KeyState::Down), (code, KeyState::Up)])
        .collect();
        expected.extend([
            (42, KeyState::Down),
            (30, KeyState::Down),
            (30, KeyState::Up),
            (42, KeyState::Up),
            (28, KeyState::Down),
            (28, KeyState::Up),
        ]);
        assert_eq!(events, expected);
    }

    #[test]
    fn invalid_sequence_prevents_all_actions_from_executing() {
        let mut events = Vec::new();
        let mut mouse = Controller::new(NoMouse);
        let mut keyboard = KeyboardController::new(RecordingKeyboard(&mut events));
        let actions = [
            Action::KeyTap { key: "a".into() },
            Action::KeySequence {
                keys: vec!["b".into(), "ctrl+code:65535".into()],
            },
        ];

        assert!(execute_actions(&mut mouse, &mut keyboard, None, &actions).is_err());
        assert!(events.is_empty());
    }

    #[test]
    fn key_sequences_are_bounded_and_validate_every_hotkey() {
        for keys in [
            vec![],
            vec!["a".into(); 257],
            vec!["a".into(), "".into()],
            vec!["a".into(), "ctrl+unknown".into()],
        ] {
            assert!(validate_actions(&[Action::KeySequence { keys }]).is_err());
        }
        assert!(
            validate_actions(&[Action::KeySequence {
                keys: vec!["a".into(); 256],
            }])
            .is_ok()
        );
        assert!(validate_actions(&vec![Action::KeyTap { key: "a".into() }; 17]).is_err());
    }

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
            Command::Agent { frame: Some(frame), .. } if frame == *"recording.mp4"
        ));
    }

    #[test]
    fn agent_accepts_a_live_directory_with_the_frames_flag() {
        let cli = Cli::try_parse_from([
            "computeruse",
            "agent",
            "--frames",
            "frames",
            "--instructions",
            "steps.txt",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Agent { frames: Some(frames), mouse_control: MouseControl::Proportional, .. } if frames == *"frames"
        ));
    }

    #[test]
    fn direct_mouse_control_requires_an_explicit_selection() {
        let cli = Cli::try_parse_from([
            "computeruse",
            "agent",
            "--frames",
            "frames",
            "--instructions",
            "steps.txt",
            "--execute",
            "--mouse-control",
            "direct",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Agent {
                mouse_control: MouseControl::Direct,
                execute: true,
                ..
            }
        ));
        assert!(
            Cli::try_parse_from([
                "computeruse",
                "agent",
                "--frames",
                "frames",
                "--instructions",
                "steps.txt",
                "--mouse-control",
                "unknown",
            ])
            .is_err()
        );
    }

    #[test]
    fn agent_rejects_multiple_frame_sources() {
        assert!(
            Cli::try_parse_from([
                "computeruse",
                "agent",
                "--frame",
                "recording.mp4",
                "--frames",
                "frames",
                "--instructions",
                "steps.txt",
            ])
            .is_err()
        );
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
