# ⚡ Pulsar — Funscript Generator (`pulsar`)

[![Buy Me A Coffee](https://img.shields.io/badge/Buy%20Me%20a%20Coffee-Donate-orange?style=for-the-badge&logo=buy-me-a-coffee)](https://buymeacoffee.com/thesmartestgooner)

> **The Neural Kinematic Workstation & Funscript Generator**  
> *Pure Native Rust Engine • Adaptive Fast/Slow Vision-to-Motion • 6-DOF Robotic Kinematics • Zero External C Dependencies*

Pulsar is a next-generation workstation and CLI engine for automated visual and acoustic motion tracking, robotic kinematic synthesis (OSR2/SR6), and funscript authoring.

---

## Features

- **Phase 8 Adaptive Fast/Slow ML Architecture**: Combines SAM 3.1 open-vocabulary semantic grounding, YOLO26 direct NMS/DFL-free top-$K$ tensor decoding, TAPNext++ sparse trajectory tracking with forward-backward cycle consistency, and Bayesian multi-hypothesis occlusion resolution.
- **3D Robotic Rig Simulator**: Real-time 60+ FPS interactive 3D forward-kinematics simulator for OSR2 and SR6 multi-axis robotic hardware with T-Code v0.3 protocol compliance.
- **Sub-Millisecond Audio Waterfall Spectrogram**: Real-time STFT multi-band frequency decomposition (Sub-Bass, Mid, High), beat-snapping, and automated haptic infill synthesis.
- **Interactive Desktop Timeline GUI Editor**: Built with `egui` / `eframe` for ultra-responsive native rendering, live keyframe dragging, pan/zoom, visual motor speed violation heatmaps, and integrated Script Doctor auditing panel.
- **Blazing Fast**: Pure Rust multi-threaded execution utilizing `rayon` for data-parallel optical flow, jemalloc global allocation, and direct tensor decoders.
- **Robust Video Ingestion**: Decodes any video format (AV1, HEVC, H.264, VP9) via hardware-accelerated FFmpeg pipe streaming (`-hwaccel auto`).
- **Deterministic Kinematics & Thermal Protection**: Jerk-limited quintic S-curves, continuous motor wattage dissipation simulation, and dynamic velocity throttling.
- **Stash Media Server Integration & Headless Batch**: Direct GraphQL querying of scenes missing interactive scripts, automated generation, and background batch queues.

---

## Installation & Quick Start

### Quick Install (Linux / macOS)
```bash
./install.sh
```
This builds the release binary with maximum optimizations and symlinks `pulsar` (and legacy aliases `fs-hub`, `open-fungen`) into `~/.local/bin/`.

---

## Usage

### 1. Interactive Desktop GUI Editor
Launch the GUI editor directly with no arguments or using the `gui` subcommand:
```bash
pulsar
# or
pulsar gui
```
Features inside the GUI:
- Open, inspect, edit, and save `.funscript` files.
- Interactive timeline with pan, zoom, point selection, point dragging, and point insertion/deletion.
- Visual speed warnings for transitions exceeding motor velocity limits ($> 450 \text{ units/s}$).
- Built-in video generator launcher with progress display.
- Real-time Script Doctor linter sidebar.

### 2. Generate a Funscript from Video (CLI)

#### Adaptive Fast/Slow Neural Mode:
```bash
pulsar generate video.mp4 \
  --model ~/.cache/pulsar/models/FunGen-12n-pov-1.1.0.onnx \
  --profile balanced \
  --fps 30.0 \
  --pov \
  --multi-axis
```

#### Optical Flow Mode:
```bash
pulsar generate video.mp4 --fps 30.0 --pov
```

#### CLI Options:
- `<VIDEO>`: Input video file.
- `-o, --output <PATH>`: Destination `.funscript` file (defaults to `<video_name>.funscript`).
- `--model <PATH>`: Path to ONNX neural tracking model (e.g. YOLOv12n / YOLO26). Automatically enables hybrid neural tracking.
- `--profile <PROFILE>`: Adaptive ML execution profile: `economy`, `balanced`, `default`, `dense`, `geometry3d`.
- `--conf <FLOAT>`: Detection confidence threshold for neural tracking (default: `0.35`).
- `--fps <FLOAT>`: Processing target frame rate (default: `30.0`).
- `--width <INT>`, `--height <INT>`: Processing grid dimensions (default: `256` for optical flow, `640` for neural).
- `--detrend-window <SECONDS>`: Window for polynomial baseline detrending (default: `2.0`).
- `--norm-window <SECONDS>`: Window for rolling min-max normalization (default: `3.0`).
- `--pov`: Enables POV mode (anchors interaction center to bottom-center).
- `--vr`: Enables VR mode (crops bottom-left quadrant for VR 180 stereo).
- `--multi-axis`: Automatically derives companion 6-DOF channels (Surge, Sway, Pitch, Roll).

### 3. Audit a Funscript with Script Doctor
```bash
pulsar doctor video.funscript --max-speed 450.0
```

### 4. Automatically Repair a Funscript
```bash
pulsar fix video.funscript -o video.fixed.funscript
```

### 5. Multi-Band Audio Spectral Infill
```bash
pulsar audio-synth video.mp4 --mode infill --band sub-bass
```

### 6. Probe Video Details
```bash
pulsar info video.mp4
```

---

## Running Tests
```bash
cargo test
```

---

## License

Pulsar is currently licensed under the **GNU Affero General Public License v3.0** ([LICENSE](LICENSE)) during pre-1.0 development.

> **Roadmap Note:** Pulsar will explicitly transition to the **Apache License, Version 2.0 (Apache-2.0)** upon the official 1.0 General Availability release.
