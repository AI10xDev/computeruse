# Quickstart

`computeruse` captures and controls the mouse and keyboard on Linux. Its visual
agent sends video frames and GUI instructions to GPT Astra, then prints the
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

## 4. Run The GPT Astra Agent

Record an MP4 (or another format supported by FFmpeg) and write one or more GUI
instructions:

```bash
printf '%s\n' 'Open the browser settings page.' > gui-steps.txt
export AZURE_OPENAI_API_KEY=...
export AZURE_OPENAI_ENDPOINT=https://RESOURCE.services.ai.azure.com/api/projects/PROJECT
```

When using `record.sh`, those values can instead be stored in the ignored local
`ast` file with the API key on line 1 and endpoint on line 3.

FFmpeg VA-API video acceleration is attempted by default for recording and
offline frame extraction. Use software explicitly when troubleshooting:

```bash
export COMPUTERUSE_VIDEO_ACCEL=off
```

To require a particular Intel or AMD adapter instead, identify its DRM render
node and run:

```bash
export COMPUTERUSE_VIDEO_ACCEL=vaapi
export COMPUTERUSE_VAAPI_DEVICE=/dev/dri/renderD128
```

The user needs read and write permission on that render node and an appropriate
VA-API driver. `auto` reports probe or decode failures and falls back to
software; `vaapi` reports an error. Render-node numbering is not a GPU identity,
so verify it with `udevadm`, `/sys/class/drm`, or `lspci` on multi-GPU systems.
This accelerates supported video encoding and decoding only, not PNG processing
or remote model inference.

Start with dry-run mode. It prints JSON decisions but does not control input:

```bash
./target/release/computeruse agent \
  --frame ./recording.mp4 \
  --instructions ./gui-steps.txt \
  --trace ./trajectory.jsonl
```

The default model is `gpt-6-astra`. If your compatible endpoint exposes it under
a different name, pass `--model NAME`.

After reviewing the dry-run output, add `--execute` to allow the model-selected
mouse and keyboard actions. Keep an emergency stop method available and only
run trusted instructions.
