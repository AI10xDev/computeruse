use anyhow::{Context, Result, bail};
use base64::Engine;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

const MAX_FRAME_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Frame {
    pub path: PathBuf,
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
}

impl Frame {
    fn data_url(&self) -> String {
        format!(
            "data:{};base64,{}",
            self.media_type,
            base64::engine::general_purpose::STANDARD.encode(&self.bytes)
        )
    }
}

pub struct FrameSource {
    directory: PathBuf,
    seen: HashMap<PathBuf, FrameIdentity>,
    trajectory: VecDeque<Frame>,
    trajectory_len: usize,
}

/// An ordered, finite source used for frame-by-frame processing of a video.
pub struct FrameSequence {
    paths: VecDeque<PathBuf>,
    trajectory: VecDeque<Frame>,
    trajectory_len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FrameIdentity {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

impl FrameSource {
    pub fn new(directory: impl Into<PathBuf>, trajectory_len: usize) -> Result<Self> {
        let directory = directory.into();
        if !directory.is_dir() {
            bail!("frame directory does not exist: {}", directory.display());
        }
        if trajectory_len == 0 {
            bail!("trajectory length must be greater than zero");
        }
        Ok(Self {
            directory,
            seen: HashMap::new(),
            trajectory: VecDeque::new(),
            trajectory_len,
        })
    }

    pub fn next(&mut self, timeout: Duration) -> Result<Option<&VecDeque<Frame>>> {
        let deadline = Instant::now() + timeout;
        loop {
            let paths = self.unseen_paths()?;
            if !paths.is_empty() {
                let keep_from = paths.len().saturating_sub(self.trajectory_len);
                for (path, identity) in &paths[..keep_from] {
                    self.seen.insert(path.clone(), *identity);
                }
                let mut loaded = false;
                for (path, identity) in paths.into_iter().skip(keep_from) {
                    match read_stable_frame(&path, identity)? {
                        Some(frame) => {
                            self.seen.insert(path, identity);
                            self.trajectory.push_back(frame);
                            loaded = true;
                        }
                        None => continue,
                    }
                }
                if loaded {
                    while self.trajectory.len() > self.trajectory_len {
                        self.trajectory.pop_front();
                    }
                    return Ok(Some(&self.trajectory));
                }
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn unseen_paths(&self) -> Result<Vec<(PathBuf, FrameIdentity)>> {
        let mut paths = fs::read_dir(&self.directory)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter_map(|path| {
                let metadata = fs::metadata(&path).ok()?;
                let identity = FrameIdentity {
                    len: metadata.len(),
                    modified: metadata.modified().ok(),
                };
                (media_type(&path).is_some() && self.seen.get(&path) != Some(&identity))
                    .then_some((path, identity))
            })
            .collect::<Vec<_>>();
        paths.sort_by_cached_key(|(path, identity)| (identity.modified, path.clone()));
        Ok(paths)
    }
}

impl FrameSequence {
    pub fn new(paths: impl IntoIterator<Item = PathBuf>, trajectory_len: usize) -> Result<Self> {
        if trajectory_len == 0 {
            bail!("trajectory length must be greater than zero");
        }
        let mut paths = paths.into_iter().collect::<Vec<_>>();
        paths.sort();
        if paths.is_empty() {
            bail!("video contains no frames");
        }
        Ok(Self {
            paths: paths.into(),
            trajectory: VecDeque::new(),
            trajectory_len,
        })
    }

    pub fn advance(&mut self) -> Result<Option<&VecDeque<Frame>>> {
        let Some(path) = self.paths.pop_front() else {
            return Ok(None);
        };
        let metadata = fs::metadata(&path)
            .with_context(|| format!("cannot inspect frame {}", path.display()))?;
        let identity = FrameIdentity {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        };
        let frame = read_stable_frame(&path, identity)?
            .with_context(|| format!("extracted frame is incomplete: {}", path.display()))?;
        self.trajectory.push_back(frame);
        while self.trajectory.len() > self.trajectory_len {
            self.trajectory.pop_front();
        }
        Ok(Some(&self.trajectory))
    }
}

fn read_stable_frame(path: &Path, identity: FrameIdentity) -> Result<Option<Frame>> {
    if identity.len > MAX_FRAME_BYTES {
        bail!(
            "frame exceeds {} MiB limit: {}",
            MAX_FRAME_BYTES / 1024 / 1024,
            path.display()
        );
    }
    let mut bytes = Vec::with_capacity(identity.len as usize);
    File::open(path)
        .with_context(|| format!("cannot open frame {}", path.display()))?
        .take(MAX_FRAME_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("cannot read frame {}", path.display()))?;
    if bytes.len() as u64 > MAX_FRAME_BYTES {
        bail!(
            "frame exceeds {} MiB limit: {}",
            MAX_FRAME_BYTES / 1024 / 1024,
            path.display()
        );
    }
    let metadata = fs::metadata(path)?;
    let after = FrameIdentity {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    };
    let media_type = media_type(path).expect("path was filtered by media type");
    if identity != after
        || bytes.len() as u64 != after.len
        || !is_complete_image(&bytes, media_type)
    {
        return Ok(None);
    }
    Ok(Some(Frame {
        bytes,
        path: path.to_path_buf(),
        media_type,
    }))
}

fn is_complete_image(bytes: &[u8], media_type: &str) -> bool {
    match media_type {
        "image/png" => {
            bytes.starts_with(b"\x89PNG\r\n\x1a\n")
                && bytes.len() >= 20
                && &bytes[bytes.len() - 12..bytes.len() - 4] == b"\0\0\0\0IEND"
        }
        "image/jpeg" => bytes.starts_with(b"\xff\xd8") && bytes.ends_with(b"\xff\xd9"),
        "image/webp" => {
            bytes.len() >= 12
                && bytes.starts_with(b"RIFF")
                && &bytes[8..12] == b"WEBP"
                && u32::from_le_bytes(bytes[4..8].try_into().expect("four-byte slice")) as usize + 8
                    == bytes.len()
        }
        _ => false,
    }
}

fn media_type(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    MouseMove { dx: i32, dy: i32 },
    MouseMoveTo { x: u16, y: u16 },
    MouseClick { button: crate::Button },
    Scroll { delta: i32, horizontal: bool },
    KeyTap { key: String },
    Hotkey { keys: String },
    Wait { milliseconds: u64 },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentDecision {
    #[serde(default)]
    pub rationale: String,
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub progress: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Transition {
    pub step: usize,
    pub frame: PathBuf,
    pub actions: Vec<Action>,
    pub progress: f32,
    pub reward: f32,
    pub completed: bool,
    pub executed: bool,
}

pub trait Policy {
    fn decide(
        &self,
        instructions: &str,
        trajectory: &VecDeque<Frame>,
        transitions: &[Transition],
    ) -> Result<AgentDecision>;
}

pub struct GptAstraPolicy {
    client: Client,
    endpoint: String,
    api_key: String,
    model: String,
}

impl GptAstraPolicy {
    pub fn from_env(model: impl Into<String>) -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .context("OPENAI_API_KEY is required for the agent command")?;
        let base_url =
            std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());
        Ok(Self {
            client: Client::builder()
                .timeout(Duration::from_secs(120))
                .build()?,
            endpoint: format!("{}/chat/completions", base_url.trim_end_matches('/')),
            api_key,
            model: model.into(),
        })
    }
}

impl Policy for GptAstraPolicy {
    fn decide(
        &self,
        instructions: &str,
        trajectory: &VecDeque<Frame>,
        transitions: &[Transition],
    ) -> Result<AgentDecision> {
        let mut content = vec![json!({
            "type": "text",
            "text": format!(
                "GUI instructions:\n{instructions}\n\nPrevious transitions:\n{}",
                serde_json::to_string(transitions)?
            )
        })];
        for (index, frame) in trajectory.iter().enumerate() {
            content.push(json!({
                "type": "text",
                "text": format!("Trajectory frame {index}: {}", frame.path.display())
            }));
            content.push(json!({
                "type": "image_url",
                "image_url": { "url": frame.data_url(), "detail": "high" }
            }));
        }
        let body = json!({
            "model": self.model,
            "messages": [
                {
                    "role": "system",
                    "content": "Act as a cautious GUI policy. Infer state changes from the ordered frames and return JSON only. Use normalized 0..65535 coordinates for mouse_move_to. Prefer one small, reversible action per observation. Never invent an action type. Set completed only when the latest frame visibly proves the instructions are complete. Output: {\"rationale\":string,\"actions\":[action],\"completed\":bool,\"progress\":number}. Action types: mouse_move(dx,dy), mouse_move_to(x,y), mouse_click(button), scroll(delta,horizontal), key_tap(key), hotkey(keys), wait(milliseconds). Buttons: left, right, middle, side, extra. Named keys and hotkeys use forms such as enter, tab, escape, ctrl+shift+a. Relative movement is limited to +/-32767, scrolling to +/-100, and waits to 30000 milliseconds."
                },
                { "role": "user", "content": content }
            ],
            "response_format": { "type": "json_object" },
            "temperature": 0.1
        });
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .context("gpt-Astra policy request failed")?;
        let status = response.status();
        let value: Value = response.json().context("policy response was not JSON")?;
        if !status.is_success() {
            bail!("policy request returned {status}: {value}");
        }
        let content = value
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .context("policy response did not contain choices[0].message.content")?;
        let decision: AgentDecision = parse_decision(content)
            .with_context(|| format!("policy returned an invalid decision: {content}"))?;
        if !(0.0..=1.0).contains(&decision.progress) {
            bail!("policy progress must be between 0 and 1");
        }
        Ok(decision)
    }
}

fn parse_decision(content: &str) -> Result<AgentDecision, serde_json::Error> {
    let trimmed = content.trim();
    let json = if trimmed.starts_with("```") {
        let without_opening = trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```JSON"))
            .or_else(|| trimmed.strip_prefix("```"))
            .unwrap_or(trimmed);
        without_opening
            .strip_suffix("```")
            .unwrap_or(without_opening)
            .trim()
    } else {
        trimmed
    };
    serde_json::from_str(json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn action_protocol_round_trips() {
        let decision = AgentDecision {
            rationale: "focus the field".into(),
            actions: vec![Action::Hotkey {
                keys: "ctrl+l".into(),
            }],
            completed: false,
            progress: 0.25,
        };
        let json = serde_json::to_string(&decision).unwrap();
        let parsed: AgentDecision = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed.actions[0], Action::Hotkey { .. }));
    }

    #[test]
    fn recognizes_supported_frame_types() {
        assert_eq!(media_type(Path::new("frame.PNG")), Some("image/png"));
        assert_eq!(media_type(Path::new("notes.txt")), None);
    }

    #[test]
    fn accepts_fenced_policy_json() {
        let decision =
            parse_decision("```json\n{\"actions\":[],\"completed\":true,\"progress\":1.0}\n```")
                .unwrap();
        assert!(decision.completed);
    }

    #[test]
    fn frame_source_starts_with_latest_bounded_trajectory() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("computeruse-frames-{suffix}"));
        fs::create_dir(&directory).unwrap();
        for name in ["001.png", "002.png", "003.png"] {
            fs::write(directory.join(name), fake_png(name.as_bytes()[0])).unwrap();
            thread::sleep(Duration::from_millis(2));
        }

        let mut source = FrameSource::new(&directory, 2).unwrap();
        let trajectory = source.next(Duration::ZERO).unwrap().unwrap();
        assert_eq!(trajectory.len(), 2);
        assert!(trajectory[0].path.ends_with("002.png"));
        assert!(trajectory[1].path.ends_with("003.png"));
        assert!(source.next(Duration::ZERO).unwrap().is_none());

        thread::sleep(Duration::from_millis(2));
        let changed = fake_png(b'x');
        fs::write(directory.join("003.png"), &changed).unwrap();
        let trajectory = source.next(Duration::ZERO).unwrap().unwrap();
        assert_eq!(trajectory.back().unwrap().bytes, changed);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn frame_sequence_processes_every_frame_in_filename_order() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("computeruse-sequence-{suffix}"));
        fs::create_dir(&directory).unwrap();
        let paths = ["frame-0000000001.png", "frame-0000000000.png"].map(|name| {
            let path = directory.join(name);
            fs::write(&path, fake_png(name.as_bytes()[17])).unwrap();
            path
        });

        let mut source = FrameSequence::new(paths, 1).unwrap();
        assert!(
            source
                .advance()
                .unwrap()
                .unwrap()
                .back()
                .unwrap()
                .path
                .ends_with("frame-0000000000.png")
        );
        assert!(
            source
                .advance()
                .unwrap()
                .unwrap()
                .back()
                .unwrap()
                .path
                .ends_with("frame-0000000001.png")
        );
        assert!(source.advance().unwrap().is_none());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_incomplete_image_containers() {
        assert!(!is_complete_image(b"\x89PNG\r\n\x1a\npartial", "image/png"));
        assert!(!is_complete_image(b"\xff\xd8partial", "image/jpeg"));
        assert!(!is_complete_image(b"RIFF\x10\0\0\0WEBP", "image/webp"));
    }

    fn fake_png(marker: u8) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend([marker; 4]);
        bytes.extend(b"\0\0\0\0IEND\0\0\0\0");
        bytes
    }
}
