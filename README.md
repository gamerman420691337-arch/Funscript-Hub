# ⚡ Pulsar — Neural Kinematic Workstation & Funscript Generator

[![CI](https://github.com/gamerman420691337-arch/pulsar-fs/actions/workflows/ci.yml/badge.svg)](https://github.com/gamerman420691337-arch/pulsar-fs/actions/workflows/ci.yml)
[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](https://www.gnu.org/licenses/agpl-3.0)
[![Rust: 1.80+](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org)
[![Buy Me A Coffee](https://img.shields.io/badge/Buy%20Me%20a%20Coffee-Donate-orange?style=for-the-badge&logo=buy-me-a-coffee)](https://buymeacoffee.com/thesmartestgooner)

> **The Next-Generation Motion Tracking Engine & Haptic Workstation**  
> *Pure Native Rust Engine • Adaptive Fast/Slow Vision-to-Motion • 6-DOF Robotic Kinematics • Zero External C Dependencies*

Pulsar is a high-performance workstation and CLI suite for automated visual and acoustic motion tracking, robotic kinematic synthesis (OSR2, SR6, DIY T-Code, Buttplug.io, The Handy), and interactive funscript authoring.

## Product specification and major roadmap

The [software specification](docs/PULSAR_SOFTWARE_SPECIFICATION.md) records the agreed product contract and chat decision sources. The [major roadmap](docs/PULSAR_MAJOR_ROADMAP.md) defines staged implementation, beginning with bounding-box, tracking, and stroke correctness. These documents distinguish requirements from measured capabilities; feature descriptions below are not release-qualification evidence.

---

## 🚀 Key Features

- **Adaptive Fast/Slow ML Architecture**: Combines SAM 3.1 open-vocabulary semantic segmentation, YOLO26 direct NMS/DFL-free top-$K$ tensor decoding, TAPNext++ sparse point tracking with forward-backward cycle consistency, and Bayesian multi-hypothesis occlusion resolution.
- **Live Gestural Motion Recording ("Puppeteering")**: Real-time interactive funscript capture in both Studio Editor and Cinema Player. Puppet motions directly via mouse Y tracking over the video canvas or tactile vertical slider strip with scroll-wheel support across variable playback speeds (`0.25x`, `0.5x`, `1.0x`, `1.5x`, `2.0x`). Includes automatic inflection point extraction, RDP curve simplification, non-destructive undo/redo splicing, and live hardware mirroring.
- **Hardware Ecosystem & Live Player**: Direct real-time streaming to USB Serial COM ports (T-Code v0.3), Intiface Central / Buttplug.io v3 WebSockets (hundreds of Bluetooth LE & Wi-Fi toys), The Handy cloud API, and DeoVR/HereSphere VR headsets.
- **3D Robotic Rig Simulator**: Real-time 60+ FPS interactive 3D forward-kinematics simulator for OSR2 and SR6 multi-axis hardware with servo limit visualization.
- **Sub-Millisecond Audio Waterfall Spectrogram**: Real-time STFT 16-band perceptual frequency decomposition (Sub-Bass, Mid, High), beat-snapping, and automated haptic infill synthesis.
- **Interactive Desktop Timeline GUI Editor & Cinema Player**: Built with `egui` / `eframe` for ultra-responsive native rendering, live keyframe dragging, pan/zoom, visual motor speed violation heatmaps, integrated Script Doctor auditing panel, and distraction-free cinema mode.
- **Hardware Benchmark**: Built-in `pulsar bench` utility measuring Lucas-Kanade optical flow throughput (3,000+ FPS), FFT transforms (700,000+ transforms/sec), and quintic kinematics evaluations (800M+ ops/sec).
- **Automated Model Manager**: Zero-config auto-download of default SOTA YOLO models, local model catalog discovery, and global caching in `~/.cache/pulsar/models/`.
- **Stash Media Server Integration & Headless Batch**: Direct GraphQL querying of scenes missing interactive scripts, automated generation, scene auto-tagging, and background batch queues.
- **Deterministic Kinematics & Thermal Protection**: Jerk-limited quintic S-curves, continuous motor wattage dissipation simulation, and dynamic velocity throttling.

---

## 🛠️ Installation & Quick Start

### Quick Install (Linux / macOS)
```bash
git clone https://github.com/gamerman420691337-arch/pulsar-fs.git pulsar
cd pulsar
./install.sh
```
This compiles the release binary with maximum native CPU optimizations and links `pulsar` into `~/.local/bin/`.

### Cross-Platform Cargo Build
```bash
cargo build --release
```

---

## 📖 CLI Command Reference

### 1. Launch Interactive Desktop GUI
```bash
pulsar
# or explicitly
pulsar gui
```
*(Press `?` or `Ctrl+H` inside the GUI to open the interactive keyboard & mouse shortcuts cheat sheet).*

#### 🎬 Live Puppeteering & Motion Recording (Studio & Cinema Modes)
Pulsar allows authoring and refining funscripts in real time as you watch a video:
- **Arm / Disarm Recording**: Press `R` or click `🔴 Record (R)` in the transport bar.
- **Puppet via Mouse Tracking**: Hover or click-drag over the video viewport. A glowing laser guide line tracks your vertical position (`0–100%`) with live badge telemetry.
- **Puppet via Tactile Slider**: Use the vertical slider strip on the right side of the canvas or hover and spin the mouse scroll wheel.
- **Slow-Motion Capture**: Drop playback speed to `0.25x` or `0.5x` for hyper-precise tracking during fast or complex scenes.
- **Curve Simplification**: Recorded samples are processed with automatic directional peak/valley detection and Ramer-Douglas-Peucker (RDP) filtering to produce clean, crisp keyframes without micro-jitters.
- **Physical Movement Percentage & Selectable Axis Slider/Knob**: Real-time visualization of actual physical servo stroke percentage (`0–100%`) and knob position gliding live during playback. Switch between Stroke (`L0`), Surge (`L1`), Sway (`L2`), Roll (`R0`), Pitch (`R1`), Twist (`R2`), and Suction (`V0`) directly from the slider dropdown.
- **Cursor-Centric Timeline Navigation**: Zooming in/out via the mouse scroll wheel locks precisely onto the exact millisecond under the mouse cursor without drifting. Zoom-out duration is clamped to the content duration (+5% margin), preventing empty infinite zoom and keeping scripts perfectly framed.
- **Undo / Redo Support**: Any recording take cleanly splices into the script over the recorded interval and can be undone with `Ctrl+Z`.

### 2. Generate Funscripts from Video

#### Zero-Config Automatic Generation:
```bash
# Automatically downloads default SOTA neural model if not present locally
pulsar generate video.mp4 --multi-axis
```

#### Neural Mode with Specific Profile:
```bash
pulsar generate video.mp4 \
  --profile balanced \
  --fps 30.0 \
  --pov \
  --multi-axis \
  --cuts scene_boundaries.json
```

#### Optical Flow Mode:
```bash
pulsar generate video.mp4 --profile economy --fps 30.0 --pov
```

| Flag | Description | Default |
|------|-------------|---------|
| `<VIDEO>` | Path to input video file | *(Required)* |
| `-o, --output <PATH>` | Output `.funscript` file destination | `<video>.funscript` |
| `--model <PATH>` | Custom ONNX neural vision model | Auto-detected |
| `--profile <NAME>` | Adaptive ML profile (`economy`, `balanced`, `default`, `dense`, `geometry3d`) | `default` |
| `--conf <FLOAT>` | Neural detection confidence threshold | `0.35` |
| `--fps <FLOAT>` | Processing target frame rate | `30.0` |
| `--cuts <PATH>` | Export detected scene boundaries to JSON | `None` |
| `--pov` | Anchor motion to bottom-center (POV perspective) | `false` |
| `--vr` | VR 180 stereo tracking mode | `false` |
| `--multi-axis` | Export full 6-DOF companion bundle (`.surge.funscript`, `.sway`, etc.) | `false` |

---

### 3. Real-Time Hardware Streaming (`pulsar play`)

Stream any funscript directly to your physical hardware in real time without third-party players:

```bash
# Stream to USB Serial T-Code microcontroller (OSR2 / SR6 / ESP32)
pulsar play video.mp4 --serial /dev/ttyUSB0 --baud 115200

# Stream to Intiface Central / Buttplug.io (Bluetooth toys)
pulsar play video.mp4 --buttplug ws://127.0.0.1:12345

# Stream to The Handy
pulsar play video.mp4 --handy <YOUR_CONNECTION_KEY>
```

---

### 4. Neural Model Manager (`pulsar model`)

Manage local and remote neural vision models directly from the CLI:

```bash
# List all registered and locally cached models
pulsar model list

# Download a specific model into ~/.cache/pulsar/models/
pulsar model download yolo26n

# View cache footprint and directory statistics
pulsar model status
```

---

### 5. Hardware Performance Benchmark (`pulsar bench`)

Benchmark your CPU and GPU compute capabilities across optical flow, FFT, and kinematics engines:

```bash
pulsar bench
```

---

### 6. Shell Autocompletions (`pulsar completions`)

Generate autocompletions for Bash, Zsh, Fish, or PowerShell:

```bash
# Bash
source <(pulsar completions bash)

# Zsh
pulsar completions zsh > ~/.zsh/completion/_pulsar

# Fish
pulsar completions fish > ~/.config/fish/completions/pulsar.fish
```

---

### 7. Library Batch Scanning & Auto-Generation

Scan an entire media directory, identify videos lacking funscripts, and batch process them:

```bash
# Scan library and display audit table
pulsar scan /path/to/media/ --recursive --missing-only

# Scan and immediately enqueue missing media for batch generation
pulsar scan /path/to/media/ --recursive --generate --multi-axis

# Dry run batch processor
pulsar batch /path/to/media/ --dry-run
```

---

### 8. Stash Media Server Synchronization

Synchronize directly with your Stash media server over GraphQL:

```bash
# Scan Stash for scenes missing funscripts
pulsar stash scan --url http://localhost:9999 --api-key <KEY>

# Generate funscripts for missing scenes, rescan metadata, and tag scenes in Stash
pulsar stash generate --url http://localhost:9999 --limit 20 --multi-axis --tag "Pulsar-Generated"
```

---

### 9. Script Doctor & Automated Repairs

Audit and repair speed violations, micro-jitters, and out-of-range positions:

```bash
# Audit a funscript for hardware speed violations (>450 units/s)
pulsar doctor video.funscript --max-speed 450.0

# Automatically clamp and smooth violating transitions
pulsar fix video.funscript -o video.fixed.funscript
```

---

### 10. Audio Spectral Infill Synthesis

Synthesize auxiliary haptic channels from video audio streams:

```bash
# Synthesize high-frequency vibration channel from bass transients
pulsar audio-synth video.mp4 --mode infill --band sub-bass --axis roll
```

---

## 🔌 Hardware Compatibility Matrix

| Hardware Type | Protocol | Communication | Supported Devices |
|---|---|---|---|
| **Multi-Axis DIY** | T-Code v0.3 | USB Serial (COM / `/dev/ttyUSB*`) | OSR2, OSR2+, SR6, ESP32, Teensy 4.0/4.1, Arduino |
| **Intiface / Buttplug** | Buttplug.io v3 | WebSocket (`ws://127.0.0.1:12345`) | 200+ commercial Bluetooth LE & Wi-Fi haptic devices |
| **The Handy** | HDSP / HSSP | HandyFeeling REST API v2 | The Handy |
| **VR Headsets** | WebSocket Sync | Telemetry JSON (`ws://localhost:11573`) | DeoVR, HereSphere, Quest, Valve Index |

---

## 🧪 Running Tests

Pulsar maintains a comprehensive test suite covering all DSP, neural inference, kinematic solves, and synchronization protocols:

```bash
cargo test
```

---

## 📄 License

Pulsar is currently licensed under the **GNU Affero General Public License v3.0** ([LICENSE](LICENSE)) during pre-1.0 development.

> **Roadmap Note:** Pulsar will explicitly transition to the **Apache License, Version 2.0 (Apache-2.0)** upon the official 1.0 General Availability release.
