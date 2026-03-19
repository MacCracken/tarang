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
