# Troubleshooting Guide

## Build errors

### "could not find `dav1d`/`openh264`/`libvpx`" system library

**Cause**: Missing system development packages for C FFI codecs.

**Fix**: Install the required packages for your enabled features:

| Feature | Arch/AGNOS | Debian/Ubuntu | Fedora |
|---------|-----------|---------------|--------|
| `dav1d` | `pacman -S dav1d` | `apt install libdav1d-dev` | `dnf install libdav1d-devel` |
| `vpx`/`vpx-enc` | `pacman -S libvpx` | `apt install libvpx-dev` | `dnf install libvpx-devel` |
| `openh264`/`openh264-enc` | (downloaded at build) | (downloaded at build) | (downloaded at build) |
| `vaapi` | `pacman -S libva` | `apt install libva-dev` | `dnf install libva-devel` |
| `pipewire` | `pacman -S pipewire` | `apt install libpipewire-0.3-dev` | `dnf install pipewire-devel` |
| `opus-enc` | `pacman -S opus` | `apt install libopus-dev` | `dnf install opus-devel` |
| `aac-enc`/`aac-dec` | `pacman -S fdk-aac` | `apt install libfdk-aac-dev` | `dnf install fdk-aac-devel` |
| `h265-decode` | `pacman -S libde265` | `apt install libde265-dev` | `dnf install libde265-devel` |

### "cros-libva build fails" / VP9 struct mismatch

**Cause**: System libva is newer than what cros-libva 0.0.13 expects.

**Fix**: The project patches cros-libva via `[patch.crates-io]` in Cargo.toml. If you see VP9 struct errors, ensure the `patches/cros-libva/` directory exists. If not, re-clone the repo.

### Feature flag confusion — "unsupported codec"

**Cause**: Codec features are opt-in. By default, no C FFI codecs are enabled.

**Fix**: Enable the features you need:
```bash
# Just audio (no FFI)
cargo build

# All codecs
cargo build --features full

# Specific codecs
cargo build --features dav1d,openh264,vpx
```

### "rav1e `paste` advisory" in cargo-deny

**Cause**: `paste` crate is unmaintained (RUSTSEC-2024-0436), transitive dep via rav1e.

**Fix**: This is suppressed in `deny.toml`. No action needed — upstream fix merged, awaiting rav1e release.

## Runtime errors

### "no VA-API render node found"

**Cause**: No GPU with VA-API support, or missing DRM render node permissions.

**Fix**:
1. Check for render nodes: `ls /dev/dri/renderD*`
2. Ensure your user is in the `video` group: `groups $USER`
3. Check VA-API support: `vainfo`

### "H.265 software decode not available"

**Cause**: H.265 requires either hardware (VA-API) or the `h265-decode` feature (LGPL).

**Fix**:
```bash
# Hardware decode (if GPU supports it)
cargo build --features vaapi,hwaccel

# Software decode (LGPL)
cargo build --features h265-decode
```

### Resampler or mixer panics on stereo buffers

**Cause (fixed in 0.20.3)**: The `num_samples` field was interpreted as total interleaved samples instead of frames. Fixed — both `resample()` and `mix_channels()` now derive frame count from data length.

### PipeWire output: "connection refused"

**Cause**: PipeWire daemon not running.

**Fix**: Start PipeWire: `systemctl --user start pipewire pipewire-pulse`

## Performance issues

### Slow resampling

Use `resample()` (linear) for real-time, `resample_sinc()` for offline/quality. See [Performance Guide](performance-guide.md).

### Large memory usage during decode

`decode_all()` loads the entire file into memory (capped at 512MB). For large files, use `next_buffer()` for streaming decode.
