# Contributing to Tarang

Thank you for your interest in contributing! This document explains how to get
started, what we expect from contributions, and how the review process works.

## Getting started

1. **Fork and clone** the repository.
2. Install the Rust toolchain (the repo pins a version in `rust-toolchain.toml`).
3. Install system dependencies (see below).
4. Run `make check` to verify everything builds and passes locally.

### System dependencies

| Dependency | Required by | Install (Arch/AGNOS) |
|---|---|---|
| **nasm** | video module (rav1e `asm` feature) | `pacman -S nasm` |
| **libvpx** | video module (vpx feature) | `pacman -S libvpx` |
| **libdav1d** | video module (dav1d feature) | `pacman -S dav1d` |
| **libva** | video module (vaapi feature) | `pacman -S libva` |
| **clang** | video module (vaapi/vpx bindgen) | `pacman -S clang` |
| **libopus** | audio module (opus-enc feature) | `pacman -S opus` |
| **libfdk-aac** | audio module (aac-enc feature) | `pacman -S libfdk-aac` |
| **pipewire** | audio module (pipewire output) | `pacman -S pipewire` |

Quick install (Arch/AGNOS):

```sh
sudo pacman -S nasm libvpx dav1d opus libfdk-aac pipewire libva clang
```

Debian/Ubuntu equivalents:

```sh
sudo apt install nasm libvpx-dev libdav1d-dev libva-dev libopus-dev \
    libfdk-aac-dev libpipewire-0.3-dev clang
```

## Development workflow

```sh
make check   # format check + clippy (zero warnings) + tests + audit
make fmt     # check formatting only
make clippy  # lint only
make test    # workspace tests + feature-gated tests
make audit   # cargo audit
make deny    # supply-chain checks (license + advisory + source)
make doc     # build rustdoc
```

CI runs the same `check` pipeline, so if it passes locally it will pass in CI.

## What to contribute

Contributions are welcome in several areas:

- **New codec backends** -- implement a codec trait, add a feature flag, register
  it in the appropriate module, and add tests.
- **Container format support** -- new demuxers/muxers under `src/demux/`.
- **Audio pipeline improvements** -- resampling, mixing, encoding, output backends.
- **AI features** -- fingerprinting, scene detection, transcription, analysis.
- **Documentation** -- rustdoc improvements, examples, guides.
- **Bug fixes** -- always welcome.

## Code style

- Run `cargo fmt` before committing. CI enforces this.
- `cargo clippy -- -D warnings` must pass with zero warnings.
- Prefer explicit types over inference in public API signatures.
- Every public item must have a doc comment (`///`).
- Keep dependencies minimal. Each codec backend must be behind a feature flag.

## Project layout

```
src/
├── lib.rs             # Crate root, module declarations, crate-level docs
├── main.rs            # CLI binary (probe, analyze, codecs, mcp)
├── core.rs            # Shared types: codecs, formats, buffers, errors
│                      #   SampleFormat, PixelFormat, AudioBuffer, VideoFrame
│                      #   yuv420p_frame_size(), validate_video_dimensions()
├── demux/             # Container parsing & muxing (all pure Rust)
│   ├── mod.rs         #   Demuxer/Muxer traits, format detection
│   ├── mp4.rs         #   MP4/M4A demux + mux
│   ├── mkv.rs         #   MKV/WebM demux + mux
│   ├── ogg.rs         #   OGG demux + mux
│   ├── wav.rs         #   WAV demux + mux
│   ├── mux.rs         #   Muxer implementations (WAV, OGG, MP4, MKV)
│   └── ebml.rs        #   EBML encoding helpers (shared MKV read/write)
├── audio/             # Audio decode, encode, resample, mix, output
│   ├── mod.rs         #   Public re-exports, AudioEncoder trait
│   ├── decode.rs      #   symphonia-based FileDecoder (pure Rust)
│   ├── encode*.rs     #   PCM, FLAC (pure Rust), Opus + AAC (C FFI)
│   ├── resample.rs    #   Linear + windowed sinc resampling (tiled LUT)
│   ├── mix.rs         #   Channel mixing (stereo/mono/5.1/generic)
│   ├── output/        #   AudioOutput trait, NullOutput, PipeWire SPSC
│   ├── sample.rs      #   Shared PCM conversion, zero-copy f32↔bytes
│   └── probe.rs       #   Format detection + metadata extraction
├── video/             # Video decode + encode via C FFI
│   ├── mod.rs         #   Unified VideoDecoder, copy_yuv420p_from_planes
│   ├── dav1d_dec.rs   #   AV1 decode (dav1d feature)
│   ├── openh264_*.rs  #   H.264 decode/encode (openh264 feature)
│   ├── vpx_*.rs       #   VP8/VP9 decode/encode (vpx feature)
│   ├── rav1e_enc.rs   #   AV1 encode (rav1e feature, pure Rust)
│   └── vaapi_*.rs     #   VA-API HW acceleration (vaapi feature)
├── ai/                # AI-powered media analysis
│   ├── mod.rs         #   Public re-exports, analyze_media()
│   ├── fingerprint.rs #   Audio fingerprinting (FFT, chroma, hashing)
│   ├── scene.rs       #   Scene boundary detection
│   ├── thumbnail.rs   #   Keyframe selection + JPEG/PNG encoding
│   ├── transcribe.rs  #   Whisper transcription routing (hoosh), chunked upload
│   └── daimon.rs      #   Vector store, RAG, LLM content description
└── mcp/               # MCP server (JSON-RPC over stdio)
    ├── mod.rs         #   Server lifecycle, tool dispatch
    └── tools.rs       #   Tool implementations
```

## Feature flags

### Video

| Feature | Codec | Backend |
|---|---|---|
| `dav1d` | AV1 decode | libdav1d (C FFI) |
| `vpx` | VP8/VP9 decode | libvpx (C FFI) |
| `vpx-enc` | VP8/VP9 encode | libvpx (C FFI) |
| `rav1e` | AV1 encode | rav1e (pure Rust + nasm) |
| `openh264` | H.264 decode | openh264 (auto-downloads) |
| `openh264-enc` | H.264 encode | openh264 (auto-downloads) |
| `vaapi` | HW accel probe/encode | libva (C FFI) |
| `full` | all codecs | all of the above + audio encoders |

### Audio

| Feature | Codec | Backend |
|---|---|---|
| `opus-enc` | Opus encode | libopus (C FFI) |
| `aac-enc` | AAC encode | libfdk-aac (C FFI) |
| `pipewire` | PipeWire output | libpipewire (C FFI) |

> Audio decoding uses symphonia (pure Rust) — no system dependencies needed.

## Adding a new codec

1. Implement the codec trait (`VideoEncoder`, `VideoDecoder`, or `AudioEncoder`)
   in a new file under `src/video/` or `src/audio/`.
2. Add a Cargo feature flag in `Cargo.toml`.
3. Gate the module with `#[cfg(feature = "your-feature")]` in the parent `mod.rs`.
4. Register the new backend in the module's `mod.rs` (add it to `supported_codecs()`
   and any dispatch logic).
5. Add tests covering creation, validation, encode/decode, and error paths.
6. Update the feature flag tables in this file and in `README.md`.

## Commit messages

- Use imperative mood: "add VP8 encoder", not "added" or "adds".
- Keep the first line under 72 characters.
- Reference issues with `#123` where applicable.

## Pull requests

- One logical change per PR.
- Include tests for new functionality.
- Update documentation if public API changes.
- PRs must pass CI (format, clippy, tests, cargo-audit).
- Maintainers may request changes before merging.

## Versioning

This project uses **strict semantic versioning** (`MAJOR.MINOR.PATCH`):

- **Pre-1.0** (current): Minor bumps (0.x → 0.y) may include breaking changes.
  Patch bumps (0.x.y → 0.x.z) are backwards-compatible.
- **Post-1.0**: Standard SemVer — breaking changes require major bump.

Version bumps are handled by maintainers using `scripts/version-bump.sh`.
Contributors do **not** need to bump the version in their PRs.

SemVer compliance is enforced via `cargo-semver-checks` in CI.

## License

By contributing you agree that your contributions will be licensed under the
[GNU General Public License v3.0](LICENSE).
