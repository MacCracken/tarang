# Migration Guide

## 0.19.3 → 0.20.3

### Breaking changes

**New `MediaInfo` field: `metadata`**

`MediaInfo` now has a `metadata: HashMap<String, String>` field. If you construct `MediaInfo` manually, add `metadata: HashMap::new()` (or `std::collections::HashMap::new()`).

```rust
// Before
let info = MediaInfo { id, format, streams, duration, file_size, title, artist, album };

// After
let info = MediaInfo { id, format, streams, duration, file_size, title, artist, album, metadata: HashMap::new() };
```

**New `TarangError` variants**

Three new error variants were added:
- `TarangError::EncodeError` — encoder failures (was `Pipeline`)
- `TarangError::MuxError` — muxer failures (was `Pipeline`)
- `TarangError::ConfigError` — validation failures (was `Pipeline`)

If you match on `TarangError`, add arms for these (or use `_` since the enum is `#[non_exhaustive]`).

**`AudioEncoder` trait: new default methods**

Two new methods with defaults were added:
```rust
fn delay_samples(&self) -> u32 { 0 }
fn padding_samples(&self) -> u32 { 0 }
```
No action needed — they have default implementations.

### New features (non-breaking)

- `hwaccel` feature: hardware accelerator detection via ai-hwaccel
- `aac-dec` feature: fdk-aac AAC decoder
- `h265-decode` feature: libde265 H.265 software decoder (LGPL)
- `AudioBuffer::convert_to(SampleFormat)`: sample format conversion
- `VideoFrame::convert_to(PixelFormat)`: pixel format conversion
- `video::scale::scale_frame()`: frame scaling
- `audio::effects`: composable audio effects pipeline
- `audio::loudness`: loudness measurement and normalization
- `FragmentedMp4Muxer`: DASH/HLS fMP4 segment generation
- `MkvMuxer::new_webm()`: audio+video WebM muxing
- `Mp4Muxer::new_with_video()`: audio+video MP4 muxing
- `EncoderConfig::builder()`: builder pattern for encoder config

### Deprecations

None.
