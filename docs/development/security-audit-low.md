# LOW Severity Security Items — 0.20.3 Audit

These 6 items were identified during the pre-release security audit and assessed
as LOW severity. Each is documented with an accept/fix decision.

## 1. RingBuffer interior mutability via raw pointer

**File**: `src/audio/output/pw.rs:62-65`
**Status**: ACCEPTED
**Rationale**: The SPSC ring buffer uses `&self` with raw pointer writes. This is
sound because `PipeWireOutput` guarantees single-producer (the `write()` method)
and single-consumer (the PipeWire callback thread). The `unsafe impl Sync` at
line 94 documents this invariant. The pattern is standard for lock-free audio I/O.

## 2. VA-API surface `.pop().unwrap()`

**File**: `src/video/vaapi_enc.rs:429`, `src/video/vaapi_dec.rs:198`
**Status**: FIXED in 0.21.3
**Fix**: Replaced with `.pop().ok_or_else(|| TarangError::HwAccelError(...))?`

## 3. Mutex lock unwrap in PipeWire output

**File**: `src/audio/output/pw.rs:165, 170`
**Status**: ACCEPTED
**Rationale**: `Mutex::lock().unwrap()` panics on poisoned mutex. Poisoning only
occurs if the PipeWire thread panicked while holding the lock. The lock guards
only a `bool` ready flag and is held for microseconds. If the PipeWire thread
panics, the audio output is already broken — panicking in the caller is acceptable.

## 4. MP4 probe info unwrap

**File**: `src/demux/mp4.rs` (probe return path)
**Status**: ACCEPTED
**Rationale**: `self.info.as_ref().unwrap()` immediately follows `self.info = Some(info)`.
Logically infallible. The unwrap cannot fail unless the assignment was skipped, which
is not possible in the current control flow.

## 5. WAV muxer data_bytes_written truncation

**File**: `src/demux/mux.rs` (WAV muxer)
**Status**: ACCEPTED
**Rationale**: WAV format has a hard 4GB limit (32-bit RIFF/data sizes). Files
approaching this limit will produce incorrect headers but no UB. The practical
limit is rarely hit — most WAV files are under 1GB. A future enhancement could
add a 4GB guard with an error return.

## 6. MP4/fMP4 sample size truncation

**File**: `src/demux/mux.rs` (sample_sizes.push)
**Status**: ACCEPTED
**Rationale**: `data.len() as u32` could truncate for samples > 4GB. Individual
audio/video samples are never this large in practice (typical: 1KB-1MB). No
real-world input triggers this path.
