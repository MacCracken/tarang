# Performance Tuning Guide

## Resampling

### Linear vs sinc interpolation

| Method | Quality | Speed | Use case |
|--------|---------|-------|----------|
| `resample()` | Good | Fast (~100x realtime) | Real-time playback, previews |
| `resample_sinc(buf, rate, 8)` | Very good | Medium (~20x realtime) | Offline processing |
| `resample_sinc(buf, rate, 32)` | Excellent | Slow (~5x realtime) | Mastering, archival |

**Recommendation**: Use `resample()` (linear) for anything interactive. Use `resample_sinc()` with `window_size=16` for final output.

### Avoiding redundant resampling

Check if rates match before resampling — both functions return the original buffer (O(1) ref-count bump) when source and target rates are identical:

```rust
// This is free when rates match:
let resampled = tarang::audio::resample(&buf, 44100)?;
```

## Decoding

### Streaming vs batch decode

| Method | Memory | Latency | Use case |
|--------|--------|---------|----------|
| `next_buffer()` | O(frame) | Low | Real-time playback, streaming |
| `decode_all()` | O(file) | High (loads entire file) | Offline analysis, encoding |

**Recommendation**: Always prefer `next_buffer()` for playback. Use `decode_all()` only for analysis tasks (fingerprinting, loudness measurement).

### Buffer size

`decode_all()` is capped at 512MB. For files larger than ~90 minutes of stereo 44.1kHz audio, use streaming decode.

## Channel mixing

`mix_channels()` is O(frames * channels). Special fast paths exist for:
- Stereo → mono (average)
- Mono → stereo (duplicate)
- 5.1 → stereo (ITU-R BS.775)
- 5.1 → mono (downmix + average)

Generic N→1 and N→2 paths handle arbitrary channel counts.

## Encoding

### Codec selection for speed

| Codec | Encode speed | Quality | Feature |
|-------|-------------|---------|---------|
| PCM | Instant | Lossless | (always available) |
| FLAC | ~50x realtime | Lossless | (always available) |
| Opus | ~20x realtime | Excellent lossy | `opus-enc` |
| AAC | ~30x realtime | Good lossy | `aac-enc` |
| H.264 (VA-API) | ~200x realtime | Good | `vaapi` (GPU) |

### Hardware encoding

VA-API encode is 5-10x faster than software for video. Enable with `--features vaapi`:

```rust
use tarang::video::{VaapiEncoder, VaapiEncoderConfig};

let config = VaapiEncoderConfig {
    codec: VideoCodec::H264,
    width: 1920, height: 1080,
    bitrate_bps: 5_000_000,
    ..Default::default()
};
let mut encoder = VaapiEncoder::new(&config)?;
let bitstream = encoder.encode(&frame)?;
```

## Memory optimization

### Zero-copy patterns

Tarang uses `bytes::Bytes` for buffer data, which is reference-counted. Operations that don't modify data (same-rate resample, same-layout mix) return a clone of the Bytes handle — O(1), no memcpy.

### Buffer reuse

Demuxers reuse internal packet buffers across reads. Encoders (Opus, AAC) pre-allocate output buffers on construction.

### Frame conversion

`VideoFrame::convert_to()` always allocates a new frame. For bulk processing, prefer converting once and reusing:

```rust
// Don't: convert every frame individually
for frame in frames {
    let rgb = frame.convert_to(PixelFormat::Rgb24)?;
}

// Do: batch processing, convert format once if possible
```

## Fingerprinting

### Frame size tuning

`FingerprintConfig::frame_size` (default 4096) controls the FFT window:
- Larger = more frequency resolution, slower
- Smaller = more time resolution, faster

For music identification, the default is optimal. For speech detection, try `frame_size: 2048`.

### Sample rate

Fingerprinting resamples to 16kHz internally. Pre-resampling avoids redundant work:

```rust
let mono_16k = tarang::audio::resample(&audio, 16000)?;
let mono = tarang::audio::mix_channels(&mono_16k, ChannelLayout::Mono)?;
let fp = compute_fingerprint(&mono, &FingerprintConfig::default())?;
```

## Loudness normalization

`measure_loudness()` is O(samples) — single-pass. `normalize_loudness()` is two-pass (measure + apply). For batch normalization, measure first, then apply:

```rust
let metrics = measure_loudness(&buf);
let gain = -14.0 - metrics.integrated_lufs;  // target -14 LUFS
let normalized = apply_gain(&buf, gain as f32)?;
```
