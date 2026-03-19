# ADR 001: Feature flags per codec backend

## Status

Accepted

## Date

2026-03-16

## Context

Tarang supports multiple video and audio codec backends, each with different
system dependencies:

- **dav1d** (AV1 decode) requires libdav1d.
- **libvpx** (VP8/VP9) requires libvpx and clang (for bindgen).
- **openh264** (H.264) auto-downloads the Cisco library at build time.
- **rav1e** (AV1 encode) requires nasm for SIMD assembly.
- **VA-API** (hardware acceleration) requires libva and a compatible GPU driver.
- **Opus/AAC encoding** requires libopus or libfdk-aac.
- **PipeWire output** requires the pipewire library.

A monolithic build that links all backends would:

1. Require every system dependency to be installed, even for users who only
   need a subset of codecs.
2. Increase the attack surface by linking unused C libraries.
3. Make cross-compilation harder (some libraries are difficult to build for
   non-native targets).
4. Bloat the binary with unused code.

## Decision

Each codec backend is gated behind a Cargo feature flag. The feature flags are:

- `tarang-video`: `dav1d`, `vpx`, `vpx-enc`, `rav1e`, `openh264`,
  `openh264-enc`, `vaapi`, `full` (all of the above).
- `tarang-audio`: `opus-enc`, `aac-enc`.

Code is conditionally compiled with `#[cfg(feature = "...")]`. The
`supported_codecs()` function and `VideoDecoder`/`VideoEncoder` dispatch logic
only include backends whose features are enabled.

A `full` convenience feature enables all backends for developers who want
everything.

## Consequences

**Positive**:

- Users only install the system dependencies they need.
- Minimal builds (e.g., demux-only) have zero C dependencies.
- The attack surface is reduced to only the linked code.
- `DecoderConfig::for_codec()` returns a clear error when a required feature
  is not compiled in, rather than a mysterious runtime failure.

**Negative**:

- CI must test multiple feature flag combinations.
- Contributors must remember to gate new code behind the appropriate feature
  and update the feature tables in documentation.
- The `Makefile` test target runs two test commands (workspace default +
  feature-gated) to ensure coverage.
