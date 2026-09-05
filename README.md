# Open-FunGen (`open-fungen`)

A high-performance, open-source cleanroom video funscript generator and editor engine written in **Rust**.

Designed as a modern, high-speed, zero-python-dependency alternative to legacy optical-flow tools, `open-fungen` provides hardware-accelerated video decoding, multi-threaded dense optical flow motion tracking, signal filtering, and automated Script Doctor auditing.

---

## Features

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
git clone https://github.com/<your-repo>/open-fungen.git
cd open-fungen
cargo build --release
```
The optimized binary will be available at `./target/release/open-fungen`.

---

## Usage

### 1. Generate a Funscript from Video
```bash
open-fungen generate video.mp4
```
Customizing options:
```bash
open-fungen generate video.mp4 \
  --fps 30.0 \
  --output custom_name.funscript \
  --detrend-window 2.0 \
  --norm-window 3.0 \
  --pov
```

#### CLI Options:
- `<VIDEO>`: Input video file.
- `-o, --output <PATH>`: Destination `.funscript` file (defaults to `<video_name>.funscript`).
- `--fps <FLOAT>`: Processing target frame rate (default: `30.0`).
- `--width <INT>`, `--height <INT>`: Processing grid dimensions (default: `256x256`).
- `--detrend-window <SECONDS>`: Window for polynomial baseline detrending (default: `2.0`).
- `--norm-window <SECONDS>`: Window for rolling min-max normalization (default: `3.0`).
- `--pov`: Enables POV mode (anchors interaction center to bottom-center).
- `--vr`: Enables VR mode (crops bottom-left quadrant for VR 180 stereo).
- `--no-balance-global`: Disables camera movement dampening.
- `--no-keyframe-reduction`: Records all frames without inflection reduction.

### 2. Audit a Funscript with Script Doctor
```bash
open-fungen doctor video.funscript
```
Audits position bounds, detects motor speed violations, identifies timing inversions, and reports average/maximum speed metrics.

### 3. Probe Video Details
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
