# Quickstart

`computeruse` captures and controls the mouse and keyboard on Linux. Its visual
agent sends video frames and GUI instructions to gpt-Astra, then prints the
model's proposed actions. Input is only injected when you add `--execute`.

## 1. Build

Install Rust 1.88 or newer and FFmpeg, clone the repository, and run:

```bash
cargo build --release
```

The binary is available at `./target/release/computeruse`.

## 2. Allow Input Access

The tool needs read access to `/dev/input` and write access to `/dev/uinput`.
For a quick local test, run it with `sudo`. For regular use, follow the safer
group setup in the [README](README.md#requirements).

## 3. Try A Command

```bash
./target/release/computeruse move 100 0
./target/release/computeruse click left
./target/release/computeruse hotkey 'ctrl+l'
```

## 4. Run The gpt-Astra Agent

Record an MP4 (or another format supported by FFmpeg) and write one or more GUI
instructions:

```bash
printf '%s\n' 'Open the browser settings page.' > gui-steps.txt
export OPENAI_API_KEY=...
export OPENAI_BASE_URL=https://api.openai.com/v1
```

Start with dry-run mode. It prints JSON decisions but does not control input:

```bash
./target/release/computeruse agent \
  --frame ./recording.mp4 \
  --instructions ./gui-steps.txt \
  --trace ./trajectory.jsonl
```

The default model is `gpt-Astra`. If your compatible endpoint exposes it under
a different name, pass `--model NAME`.

After reviewing the dry-run output, add `--execute` to allow the model-selected
mouse and keyboard actions. Keep an emergency stop method available and only
run trusted instructions.
