# Computeruse Setup Review

Interactive Python 3.10+ wizard for reviewing `AI10xDev/computeruse` installation
with an OpenCode **build** agent backed by your configured **GPT-Sol** deployment.
It produces a plan, not an automatic installer. No Python dependencies are needed.

## How to Install

**`setup.py` helps you plan the installation; it does not install computeruse.**
Do not use `pip install .`, `python setup.py install`, or `sudo python3 setup.py`.
This is an interactive wizard, not a Python packaging script.

1. Get the repository and open a terminal in its root directory:

```sh
git clone https://github.com/AI10xDev/computeruse.git
cd computeruse
python3 --version
```

Use Python 3.10 or newer. If you already have this checkout, use it instead of
cloning again. Commands below assume you are in the repository root.

2. Install/configure OpenCode with your GPT-Sol deployment as described under
   [Usage](#usage), then generate your installation plan:

```sh
python3 computeruse-setup/setup.py --output /tmp/computeruse-install-plan.md
```

Answer the OS, desktop, installation, and permission questions. When prompted for
the model, enter your actual OpenCode `provider/model` ID for GPT-Sol, not the
literal name `GPT-Sol`. Answer `yes` to fetching sources, and only approve sending
context to the model after reviewing the consent notice. Both default to `no`.
Choose a new output filename if the file already exists.

3. Open `/tmp/computeruse-install-plan.md`, resolve any compatibility warnings,
   and manually run only the installation commands you have reviewed and approved.
   The Markdown report is not a shell script; do not execute it with `bash`.

For example, if the plan confirms a Linux source installation and you already
have Rust >=1.88 and the required build tools, install the Rust binaries as your
regular user with:

```sh
cargo install --locked --git https://github.com/AI10xDev/computeruse.git \
  --rev 59f4c98a5fe10a7da91a853068738c7883485359 --root "$HOME/.local"
export PATH="$HOME/.local/bin:$PATH"
computeruse --version
computeruse --help
```

This installs the pinned Rust binaries only, not system dependencies, `record.sh`,
the Wayland launcher, API credentials, or input permissions. For Arch/Wayland,
follow the [Arch installer guide](../docs/ARCH-WAYLAND.md#install) instead if that
is your selected route. Its default package step performs a full system upgrade.

4. Follow the plan's capture, endpoint, and permission checks for your actual
   desktop before starting a GUI session. A successful `--help` is not a desktop
   test. Start without `--execute`; even preview sessions send screenshots to the
   vision endpoint. Never run the agent as root.

To inspect the wizard's request without OpenCode or a model call, use
`python3 computeruse-setup/setup.py --dry-run`. This downloads public sources after
consent and prints a prompt, not an AI-generated installation plan.

## CLI Input

Start the interactive wizard from the repository root:

```sh
python3 computeruse-setup/setup.py --output /tmp/computeruse-install-plan.md
```

Prompts are grouped into **Target Machine**, **Installation**, **Access and
Privacy**, and **Review and Consent**. Select a listed number or type the option
name (case-insensitive). Free-text answers, such as paths and model IDs, retain
their case. Press Enter to accept the displayed default, or Ctrl+C to cancel.
Two-option yes/no questions also accept `y` and `n`.

For example, enter `2` or `wayland` here:

```text
--- 1/4  Target Machine ---

  Target display session
    1. x11
    2. wayland
    3. headless
    4. unknown (default)
  > [unknown]: 2
```

On launch, a brief **0.6-second ASCII spinner** introduces the wizard before its
banner and first question. This is a visual introduction, not a system check;
it does not fetch sources, contact a model, or change your installation.

The same spinner cycles through `| / - \` while fetching GitHub sources and while
GPT-Sol generates the plan. It stops before the next input prompt; no animation
runs while you type. Completion is marked `[done]`; errors or cancellation are
marked `[stopped]`, not falsely reported as success. Status messages go to stderr,
not into the generated report. There are no extra Python dependencies.

Disable animation explicitly for accessibility or a plain terminal:

```sh
python3 computeruse-setup/setup.py --no-animation --output /tmp/computeruse-install-plan.md
```

Animation is also disabled automatically if stdin, stdout, or stderr is not a TTY,
or `TERM=dumb`. These modes skip the launch delay and still print static progress
messages. `--help` also runs immediately without animation. Safe defaults
and the separate fetch/model consent questions remain unchanged; choosing input
permissions in the plan does not grant them or start an installation.

## Review Findings

Reviewed upstream commit
[`59f4c98`](https://github.com/AI10xDev/computeruse/tree/59f4c98a5fe10a7da91a853068738c7883485359)
(v0.2.0, current main when reviewed):

- **Medium: conflicting privilege guidance.** `README.md:69-71` and
  `QUICKSTART.md:19-21` recommend sudo for quick tests, while
  `docs/ARCH-WAYLAND.md:90-96` says never run the agent as root. Avoid elevating
  the networked, model-driven workflow; grant only required device permissions.
- **Medium: possible partial setup after a kernel upgrade.**
  `scripts/install-arch-wayland.sh:72` upgrades Arch, then lines 109-115 change
  persistent permissions before loading uinput. If the running kernel's modules
  were replaced, `modprobe` can fail and leave partial installation. Reboot and
  reassess before resuming. This is a static finding, not a reproduced failure.
- **Documented compatibility limits, not bugs:** Linux, Rust >=1.88, FFmpeg for
  video processing; native X11 for `record.sh` and verified cursor control.
  The separate Arch Wayland launcher needs Python and a grim-compatible compositor.
  It does not support GNOME/KDE portals or verified Wayland pointer mapping.
- **Privacy:** dry-run desktop sessions still send screenshots to the configured
  remote vision endpoint. Input execution requires separate explicit consent.
  The app defaults to `gpt-6-astra`; GPT-Sol here is only the setup builder.

Source references are relative to the linked commit. Historical evidence in
`.spec-runs/run-ef0310f9-ae30-47d0-b261-53dda0dca2a4/play_vision.out:2376-2388`
records the released Arch installer/launcher and passing CI. Current upstream
source confirms those features already exist; this wizard does not replace them.
That log and `docs/ARCH-WAYLAND.md:178-182` leave real desktop capture, input and
endpoint compatibility unverified. No relevant completed builder wizard was found.

## Usage

Preview the request without OpenCode or model credentials:

```sh
python3 computeruse-setup/setup.py --dry-run --output /tmp/computeruse-review-prompt.txt
```

This still asks permission to download eight public source files from the pinned
commit. It fails on missing, oversized, or unreadable sources rather than silently
making a plan from incomplete evidence. No repository code is executed.

For a real review, install an OpenCode CLI supporting `run --pure --format json`
and configure your GPT-Sol provider and authentication through OpenCode's global
configuration. Project-only providers and providers supplied through `OPENCODE_CONFIG`,
`OPENCODE_CONFIG_DIR`, or `OPENCODE_CONFIG_CONTENT` are intentionally not inherited.
Find the actual globally configured provider/model identifier using
`opencode models` outside a project and without those overrides, then run:

```sh
python3 computeruse-setup/setup.py --model YOUR_PROVIDER/YOUR_GPT_SOL_ID \
  --output /tmp/computeruse-setup-plan.md
```

The identifier above is a placeholder, not a claimed public model name. Omit
`--model` to be prompted. The wizard cannot verify a deployment's underlying model;
it uses exactly the identifier you supply, without choosing a fallback. Authentication
errors or empty builder output fail the run. No actual GPT-Sol endpoint was supplied
or called during development.

Prompts cover target OS/architecture, display server and compositor, source versus
Arch installer, install prefix, privileged packages, full upgrades, preview/control/
raw recording, input permissions, remote screenshot consent, and vision provider.
Target answers override local OS hints, so running over SSH does not imply desktop
support. Defaults decline privileges and remote screenshots. EOF/Ctrl+C cancels.
Answers are not credentials; do not paste secrets into them.

Before a model call the wizard asks for explicit consent to transmit the preferences,
sources, and any automatically loaded global OpenCode instructions or custom agent
prompts (including global `AGENTS.md`/`CLAUDE.md`). Review that global context first;
tool denial does not prevent OpenCode from including it in the model request.
It uses a fresh temporary working directory and per-process OpenCode
configuration: build-agent and global tool permissions denied, sharing disabled,
external plugins/skills disabled, no project configuration, no auto-update. Build,
title, summary, and compaction agents are pinned to the selected model. This
does not modify your saved OpenCode configuration or require restarting your session.
The builder analyzes supplied evidence instead of using shell or browser tools.
OpenCode may retain the session in its normal local storage; your provider's data
retention applies. Global provider configuration is still used and must be trusted:
tool permissions are not an OS sandbox or a guarantee against startup side effects
from a locally customized CLI/configuration.

`--output` refuses to overwrite existing files. Without it the report is printed.
`--revision` accepts another full commit SHA (all source paths must exist there);
`--opencode` selects a trusted CLI executable and `--timeout` bounds the builder
request (600 seconds by default). Generated commands are advice, not validation:
review them before running anything, especially upgrades and permission changes.

## Tests

```sh
python3 -B -m unittest discover -s computeruse-setup -p 'test_*.py' -v
```

Tests mock GitHub and OpenCode. They do not install computeruse or validate a real
GPT-Sol deployment, GUI capture, or input injection.
