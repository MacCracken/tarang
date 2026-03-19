# Building Tarang

## System Requirements

| Dependency | Required By | Notes |
|---|---|---|
| **Rust** (2024 edition) | all crates | `rustup` recommended |
| **nasm** | `tarang-video` (rav1e `asm` feature) | AV1 encoder SIMD; `pacman -S nasm` |
| **libvpx** | `tarang-video` (vpx feature) | VP8/VP9 decode/encode; `pacman -S libvpx` |
| **libdav1d** | `tarang-video` (dav1d feature) | AV1 decoding; `pacman -S dav1d` |
| **libopus** | `tarang-audio` (opus-enc feature) | Opus encoding; `pacman -S opus` |
| **libfdk-aac** | `tarang-audio` (aac-enc feature) | AAC encoding; `pacman -S libfdk-aac` |
| **pipewire** | `tarang-audio` (pipewire output) | Audio output; `pacman -S pipewire` |
| **libva** | `tarang-video` (vaapi feature) | HW-accelerated video; `pacman -S libva` |
| **clang** | `tarang-video` (vaapi/vpx bindgen) | C header parsing; `pacman -S clang` |

### Quick Install (Arch/AGNOS)

```sh
sudo pacman -S nasm libvpx dav1d opus libfdk-aac pipewire libva clang
```

## Building

```sh
# Default build (all features auto-detected)
cargo build

# Full build with all codec features
cargo build --features tarang-video/full

# Release build
cargo build --release
```

## Testing

```sh
# Workspace tests (default features)
cargo test --workspace

# Video tests with all codec backends
cargo test -p tarang-video --features full

# Single crate
cargo test -p tarang-audio
```

## Feature Flags

### tarang-video
| Feature | Codec | Backend |
|---|---|---|
| `dav1d` | AV1 decode | libdav1d (C FFI) |
| `vpx` | VP8/VP9 decode | libvpx (C FFI) |
| `vpx-enc` | VP8/VP9 encode | libvpx (C FFI) |
| `rav1e` | AV1 encode | rav1e (pure Rust + nasm) |
| `openh264` | H.264 decode | openh264 (auto-downloads) |
| `openh264-enc` | H.264 encode | openh264 (auto-downloads) |
| `vaapi` | HW accel probe/encode | libva (C FFI) |
| `full` | all of the above | |

### tarang-audio
| Feature | Codec | Backend |
|---|---|---|
| `opus-enc` | Opus encode | libopus (C FFI) |
| `aac-enc` | AAC encode | libfdk-aac (C FFI) |

> Audio decoding uses symphonia (pure Rust) — no system dependencies needed for decode.

## MCP Server

```sh
# Run as MCP server on stdio (for daimon/agnoshi integration)
tarang mcp
```

## CLI

```sh
tarang probe <file>      # Show format, codec, duration, streams
tarang analyze <file>    # AI content classification
tarang codecs            # List supported codecs
```

## Debugging & Performance

Tarang uses [tracing](https://docs.rs/tracing) for structured logging. Control verbosity with `RUST_LOG`:

```sh
# Show all tarang debug logs
RUST_LOG=debug tarang probe file.wav

# Show only audio crate logs
RUST_LOG=tarang_audio=debug tarang probe file.wav

# Show trace-level video decoder details
RUST_LOG=tarang_video=trace tarang probe file.mkv

# Show everything (including dependency logs)
RUST_LOG=trace tarang probe file.wav

# Combine filters
RUST_LOG=tarang_audio=debug,tarang_demux=debug tarang probe file.mp4
```

### Performance diagnostics

Key debug-level messages for performance analysis:

| Module | Message | Fields |
|---|---|---|
| `tarang_audio::resample` | `resample complete` | `src_rate`, `dst_rate`, `src_frames`, `dst_frames`, `channels` |
| `tarang_audio::encode_flac` | `FLAC encode complete` | `frames`, `channels`, `bps`, `output_bytes` |
| `tarang_audio::decode` | `decode_all complete` | `total_samples`, `total_bytes` |
| `tarang_audio::mix` | `mix complete` | `src_channels`, `dst_channels`, `frames` |
| `tarang_demux::mp4` | `MP4 probe complete` | `format`, `streams` |
| `tarang_demux::mkv` | `MKV probe complete` | `format`, `streams` |
| `tarang_video` | `video packet sent` | `codec`, `data_len`, `pending` |
| `tarang_ai::fingerprint` | `fingerprint computed` | `hashes`, `duration_secs` |

### Timing with tracing

For timing analysis, use `tracing-timing` or wrap operations:

```sh
# Time a probe operation
time RUST_LOG=tarang_demux=debug tarang probe large_file.mkv
```
