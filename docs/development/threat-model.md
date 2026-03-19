# Threat Model

This document describes the security-relevant attack surface of tarang and the
mitigations in place for each category.

## 1. Media file parsing

**Threat**: Tarang processes untrusted media files. Malformed containers can
trigger integer overflows, oversized allocations, out-of-bounds reads, or
infinite loops.

**Attack vectors**:

- Crafted MP4 box sizes (zero, negative via underflow, or `u64::MAX`).
- MKV SimpleBlock with `size < header_size` causing unsigned underflow.
- OGG pages with invalid CRC or truncated data.
- WAV files with bitrate calculations that overflow `u32`.
- EBML variable-width integers with invalid encoding.

**Mitigations**:

- MP4 box sizes capped at 4 GB to prevent OOM.
- MKV parser validates `size >= header_size` before subtraction.
- OGG demuxer validates CRC-32 on every page.
- WAV and probe bitrate calculations use `checked_mul()` / `saturating_add()`.
- All demuxers check `bytes.len()` before indexing (e.g., MP3 magic byte
  detection requires `len >= 2`).

## 2. FFI safety (C codec libraries)

**Threat**: Video codec backends (dav1d, openh264, libvpx) are C libraries.
Bugs in these libraries -- or incorrect use of their APIs -- can cause memory
corruption, use-after-free, or crashes.

**Attack vectors**:

- Passing malformed bitstreams to C decoders.
- Trusting stride/dimension values returned by C libraries without validation.
- Incorrect lifetime management of C-allocated resources.
- ABI version mismatch between Rust bindings and system library.

**Mitigations**:

- **RAII guards**: `VpxImageGuard` ensures C resources are freed even on panic.
- **Bounds checks**: All frame data is validated before `unsafe` plane copies.
  dav1d plane strides are validated on all 3 planes before slicing.
- **Signed stride arithmetic**: VPX FFI uses `isize` for strides (handles
  negative strides correctly).
- **Pixel format validation**: dav1d rejects non-I420, VPX decoder rejects
  non-I420, openh264 encoder requires YUV420p.
- **Dimension validation**: `validate_video_dimensions()` checks non-zero and
  even dimensions before encoding.
- **Data size guards**: `data.len() > u32::MAX` rejected at VPX decoder entry.
- **ABI detection**: env-libvpx-sys generates bindings from system headers via
  bindgen, ensuring struct layouts always match the installed library.
- **`unsafe impl Send`**: Only applied to single-owner codec contexts with
  documented safety rationale.

## 3. Network (AI API calls)

**Threat**: The hoosh (Whisper) and daimon (LLM) clients make outbound HTTPS
requests. Compromised or malicious endpoints could return crafted responses.

**Attack vectors**:

- Man-in-the-middle on API connections.
- Malicious JSON responses causing panics during deserialization.
- API keys leaked in logs or error messages.

**Mitigations**:

- All HTTP clients use `reqwest` with `rustls` (no OpenSSL). TLS is mandatory.
- JSON responses are accessed via safe `.get()` chains, not unchecked indexing.
- HTTP errors are propagated (not silently swallowed) with warning logs.
- API keys are read from configuration, not hardcoded.

## 4. MCP server (JSON-RPC over stdio)

**Threat**: The MCP server accepts tool invocations from a connected agent.
Malicious or malformed requests could trigger unintended operations.

**Attack vectors**:

- Empty or malformed file paths in tool parameters.
- Oversized JSON-RPC messages.
- Requests designed to trigger expensive operations (large file analysis).

**Mitigations**:

- `require_path()` helper validates non-empty paths for all MCP tool handlers.
- The MCP server communicates only over stdio -- no network socket is opened.
- Async errors from tool handlers are logged (not silently dropped via
  `let _ =`).

## 5. Supply chain

**Threat**: Compromised or vulnerable dependencies could introduce security
issues.

**Mitigations**:

- `cargo audit` runs in CI and blocks on known vulnerabilities.
- `cargo deny` checks license compliance, advisory database, and dependency
  sources.
- `deny.toml` restricts dependencies to crates.io (no unknown registries or
  git sources).
- Dependencies are pinned in `Cargo.lock`.
- Known advisories are tracked and resolved promptly (e.g., openh264
  RUSTSEC-2025-0008, libvpx-sys RUSTSEC-2023-0018).
