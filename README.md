# Open-FunGen (`open-fungen`)

A high-performance, open-source cleanroom video funscript generator and editor engine written in **Rust**.

Designed as a modern, high-speed, zero-python-dependency alternative to legacy optical-flow tools, `open-fungen` provides hardware-accelerated video decoding, multi-threaded dense optical flow motion tracking, neural YOLOv12 anatomical detection via ONNX Runtime, signal filtering, interactive desktop timeline GUI editor (`egui`/`eframe`), and automated Script Doctor auditing.

---

## Features

- **Interactive Desktop Timeline GUI Editor**: Built with `egui` / `eframe` for ultra-responsive native rendering, live keyframe dragging, pan/zoom, visual motor speed violation heatmaps, and integrated Script Doctor auditing panel.
- **Neural ONNX Tracking Pipeline**: Built-in YOLOv12 detector running with ONNX Runtime (`ort`) supporting FunGen anatomical models (`FunGen-12n-pov-1.1.0.onnx`), 10 anatomical classes, non-maximum suppression (NMS), and adaptive probe/target tracking.
- **Blazing Fast**: Pure Rust multi-threaded execution utilizing `rayon` for data-parallel gradient optical flow.
- **Robust Video Ingestion**: Decodes any video format (AV1, HEVC, H.264, VP9) via hardware-accelerated FFmpeg pipe streaming (`-hwaccel auto`).
- **Adaptive Interaction Center**: Computes 2D velocity divergence ($\nabla \cdot \vec{v}$) to dynamically track expansion/contraction centers, with support for fixed bottom-center POV mode and VR 180 stereo quadrant cropping.
- **Scene Cut Detection**: Dual-stage scene cut detection combining photometric $L_1$ frame differences with velocity magnitude thresholding, preventing tracking drift across cuts.
- **Deterministic Signal Processing**: Overlapping Hanning-windowed linear detrending, Gaussian smoothing, and rolling min-max normalization into $[0, 100]$.
- **Keyframe Reduction**: Automatic inflection-point detection (slope inversions) for clean, hardware-friendly funscripts.
- **Integrated Script Doctor**: Built-in linter checking for hardware speed violations ($> 450 \text{ units/s}$), timing inversions, micro-jitters, and position anomalies.

---

## Installation & Building

### Prerequisites
- [Rust & Cargo](https://rustup.rs/) (1.80+)
- System `ffmpeg` and `ffprobe` binaries in your `PATH`

### Build from source
```bash
cd open-fungen
cargo build --release
```
The optimized binary will be available at `./target/release/open-fungen`.

---

## Usage

### 1. Interactive Desktop GUI Editor
Launch the GUI editor directly with no arguments or using the `gui` subcommand:
```bash
open-fungen
# or
open-fungen gui
```
Features inside the GUI:
- Open, inspect, edit, and save `.funscript` files.
- Interactive timeline with pan, zoom, point selection, point dragging, and point insertion/deletion.
- Visual speed warnings for transitions exceeding motor velocity limits ($> 450 \text{ units/s}$).
- Built-in video generator launcher with progress display.
- Real-time Script Doctor linter sidebar.

### 2. Generate a Funscript from Video (CLI)

#### Optical Flow Mode:
```bash
open-fungen generate video.mp4 --fps 30.0 --pov
```

#### Neural YOLO Mode:
```bash
open-fungen generate video.mp4 \
  --model path/to/FunGen-12n-pov-1.1.0.onnx \
  --conf 0.35 \
  --fps 30.0 \
  --pov
```

#### CLI Options:
- `<VIDEO>`: Input video file.
- `-o, --output <PATH>`: Destination `.funscript` file (defaults to `<video_name>.funscript`).
- `--model <PATH>`: Path to ONNX neural tracking model (e.g. YOLOv12n / FunGen model). Automatically enables hybrid neural tracking.
- `--conf <FLOAT>`: Detection confidence threshold for neural tracking (default: `0.35`).
- `--fps <FLOAT>`: Processing target frame rate (default: `30.0`).
- `--width <INT>`, `--height <INT>`: Processing grid dimensions (default: `256` for optical flow, `640` for neural).
- `--detrend-window <SECONDS>`: Window for polynomial baseline detrending (default: `2.0`).
- `--norm-window <SECONDS>`: Window for rolling min-max normalization (default: `3.0`).
- `--pov`: Enables POV mode (anchors interaction center to bottom-center).
- `--vr`: Enables VR mode (crops bottom-left quadrant for VR 180 stereo).
- `--no-balance-global`: Disables camera movement dampening.
- `--no-keyframe-reduction`: Records all frames without inflection reduction.

### 3. Audit a Funscript with Script Doctor
```bash
open-fungen doctor video.funscript
```
Audits position bounds, detects motor speed violations, identifies timing inversions, and reports average/maximum speed metrics.

### 4. Probe Video Details
```bash
open-fungen info video.mp4
```

---

## Running Tests
```bash
cargo test
```

---

## License
Dual-licensed under either of:
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
