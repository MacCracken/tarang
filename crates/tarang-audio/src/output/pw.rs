//! PipeWire audio output backend
//!
//! Streams decoded F32 audio to PipeWire via its Rust bindings.
//! Requires the `pipewire` feature and `libpipewire-0.3` system library.
//!
//! Architecture: the PipeWire main loop runs on a dedicated thread. The caller
//! writes F32 samples into a lock-free ring buffer. The PipeWire process callback
//! reads from the ring buffer to fill output buffers. Synchronization uses atomics
//! for the ring buffer and a condvar for flush/ready signaling.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use tarang_core::{AudioBuffer, Result, TarangError};

use super::{AudioOutput, OutputConfig};

/// Lock-free ring buffer shared between the caller and the PipeWire callback thread.
///
/// Uses atomic read/write positions so the producer (caller) and consumer
/// (PipeWire RT callback) never need to acquire a lock during normal operation.
struct RingBuffer {
    data: Vec<f32>,
    read_pos: AtomicUsize,
    write_pos: AtomicUsize,
    capacity: usize,
}

impl RingBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            data: vec![0.0; capacity],
            read_pos: AtomicUsize::new(0),
            write_pos: AtomicUsize::new(0),
            capacity,
        }
    }

    fn available(&self) -> usize {
        let w = self.write_pos.load(Ordering::Acquire);
        let r = self.read_pos.load(Ordering::Acquire);
        if w >= r { w - r } else { self.capacity - r + w }
    }

    fn free_space(&self) -> usize {
        self.capacity - 1 - self.available()
    }

    /// Producer: write samples into the ring buffer.
    /// Returns number of samples actually written.
    fn write(&self, samples: &[f32]) -> usize {
        let to_write = samples.len().min(self.free_space());
        let mut wp = self.write_pos.load(Ordering::Relaxed);
        for &sample in samples.iter().take(to_write) {
            // Safety: single producer, wp is only modified here.
            // data[wp] is not read by consumer until write_pos is updated.
            unsafe {
                let ptr = self.data.as_ptr().add(wp) as *mut f32;
                std::ptr::write(ptr, sample);
            }
            wp = (wp + 1) % self.capacity;
        }
        self.write_pos.store(wp, Ordering::Release);
        to_write
    }

    /// Consumer: read samples from the ring buffer into dst.
    /// Fills remaining dst with silence (0.0). Returns number of real samples read.
    fn read(&self, dst: &mut [f32]) -> usize {
        let to_read = dst.len().min(self.available());
        let mut rp = self.read_pos.load(Ordering::Relaxed);
        for item in dst.iter_mut().take(to_read) {
            *item = self.data[rp];
            rp = (rp + 1) % self.capacity;
        }
        self.read_pos.store(rp, Ordering::Release);
        // Fill remainder with silence
        for item in dst.iter_mut().skip(to_read) {
            *item = 0.0;
        }
        to_read
    }
}

// Safety: RingBuffer uses atomics for coordination. The data vec is only written
// by the producer (via raw pointer with atomic fence) and read by the consumer
// after the write_pos fence. This is the standard SPSC ring buffer pattern.
unsafe impl Sync for RingBuffer {}
unsafe impl Send for RingBuffer {}

/// Shared state for signaling between the main thread and the PipeWire thread.
struct PwSignal {
    ready: Mutex<bool>,
    condvar: Condvar,
}

/// PipeWire audio output sink.
///
/// The PipeWire main loop runs on a background thread. Audio samples are
/// written into a lock-free ring buffer and consumed by the PipeWire process callback.
pub struct PipeWireOutput {
    config: Option<OutputConfig>,
    ring: Arc<RingBuffer>,
    running: Arc<AtomicBool>,
    signal: Arc<PwSignal>,
    pw_thread: Option<std::thread::JoinHandle<()>>,
}

impl Default for PipeWireOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl PipeWireOutput {
    pub fn new() -> Self {
        Self {
            config: None,
            ring: Arc::new(RingBuffer::new(0)),
            running: Arc::new(AtomicBool::new(false)),
            signal: Arc::new(PwSignal {
                ready: Mutex::new(false),
                condvar: Condvar::new(),
            }),
            pw_thread: None,
        }
    }
}

impl AudioOutput for PipeWireOutput {
    fn open(&mut self, config: &OutputConfig) -> Result<()> {
        // Ring buffer sized for ~500ms of audio
        let ring_size = (config.sample_rate as usize * config.channels as usize) / 2;
        self.ring = Arc::new(RingBuffer::new(ring_size.max(16384)));
        self.running = Arc::new(AtomicBool::new(true));
        self.signal = Arc::new(PwSignal {
            ready: Mutex::new(false),
            condvar: Condvar::new(),
        });

        let ring_ref = self.ring.clone();
        let running_ref = self.running.clone();
        let signal_ref = self.signal.clone();
        let rate = config.sample_rate;
        let channels = config.channels as u32;
        let ch = config.channels as usize;

        let handle = std::thread::Builder::new()
            .name("tarang-pipewire".into())
            .spawn(move || {
                pw_thread_main(ring_ref, running_ref, signal_ref, rate, channels, ch);
            })
            .map_err(|e| TarangError::Pipeline(format!("spawn PipeWire thread: {e}")))?;

        self.pw_thread = Some(handle);
        self.config = Some(*config);

        // Wait for PipeWire thread to signal readiness (up to 2s)
        let guard = self.signal.ready.lock().unwrap();
        let (guard, timeout) = self
            .signal
            .condvar
            .wait_timeout_while(guard, Duration::from_secs(2), |ready| !*ready)
            .unwrap();
        drop(guard);
        if timeout.timed_out() {
            tracing::warn!("PipeWire init did not signal ready within 2s — proceeding anyway");
        }

        Ok(())
    }

    fn write(&mut self, buf: &AudioBuffer) -> Result<()> {
        if self.config.is_none() {
            return Err(TarangError::Pipeline("output not opened".to_string()));
        }

        let byte_len = buf.data.len();
        if !byte_len.is_multiple_of(4) {
            return Err(TarangError::Pipeline(
                "audio buffer size not aligned to f32".to_string(),
            ));
        }
        // Safety: AudioBuffer data originates from F32 serialization; heap alignment >= 8 bytes.
        debug_assert!(buf.data.as_ptr().align_offset(std::mem::align_of::<f32>()) == 0);
        let samples =
            unsafe { std::slice::from_raw_parts(buf.data.as_ptr() as *const f32, byte_len / 4) };

        let mut written = 0;
        while written < samples.len() {
            let n = self.ring.write(&samples[written..]);
            written += n;
            if written < samples.len() {
                std::thread::sleep(Duration::from_micros(200));
            }
        }

        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        // Wait for ring buffer to drain, using condvar-style polling with
        // a reasonable timeout instead of a fixed sleep loop.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if self.ring.available() == 0 {
                return Ok(());
            }
            // Brief sleep — PipeWire consumes at audio rate
            std::thread::sleep(Duration::from_millis(5));
        }
        tracing::warn!(
            remaining = self.ring.available(),
            "PipeWire flush timed out after 5s"
        );
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.pw_thread.take() {
            let _ = handle.join();
        }
        self.config = None;
        Ok(())
    }

    fn latency(&self) -> Duration {
        let avail = self.ring.available();
        if let Some(ref config) = self.config
            && config.sample_rate > 0
            && config.channels > 0
        {
            let frames = avail / config.channels as usize;
            return Duration::from_secs_f64(frames as f64 / config.sample_rate as f64);
        }
        Duration::ZERO
    }
}

impl Drop for PipeWireOutput {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

/// PipeWire main loop thread function.
///
/// All PipeWire objects are created and destroyed within this thread,
/// avoiding Send/Sync issues.
fn pw_thread_main(
    ring: Arc<RingBuffer>,
    running: Arc<AtomicBool>,
    signal: Arc<PwSignal>,
    rate: u32,
    channels: u32,
    ch: usize,
) {
    use pipewire as pw;
    use pw::spa::pod::Pod;
    use pw::spa::sys as spa_sys;

    pw::init();

    let main_loop = match pw::main_loop::MainLoopBox::new(None) {
        Ok(ml) => ml,
        Err(e) => {
            tracing::error!("PipeWire main loop: {e}");
            return;
        }
    };

    let context = match pw::context::ContextBox::new(main_loop.loop_(), None) {
        Ok(ctx) => ctx,
        Err(e) => {
            tracing::error!("PipeWire context: {e}");
            return;
        }
    };

    let core = match context.connect(None) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("PipeWire core connect: {e}");
            return;
        }
    };

    let stream = match pw::stream::StreamBox::new(
        &core,
        "tarang-audio",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_ROLE => "Music",
            *pw::keys::MEDIA_CATEGORY => "Playback",
        },
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("PipeWire stream: {e}");
            return;
        }
    };

    // Build audio format pod
    let mut audio_info = pw::spa::param::audio::AudioInfoRaw::new();
    audio_info.set_format(pw::spa::param::audio::AudioFormat::F32LE);
    audio_info.set_rate(rate);
    audio_info.set_channels(channels);

    let values: Vec<u8> = match pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(pw::spa::pod::Object {
            type_: spa_sys::SPA_TYPE_OBJECT_Format,
            id: spa_sys::SPA_PARAM_EnumFormat,
            properties: audio_info.into(),
        }),
    ) {
        Ok(r) => r.0.into_inner(),
        Err(e) => {
            tracing::error!("PipeWire pod serialize: {e}");
            return;
        }
    };

    let pod = match Pod::from_bytes(&values) {
        Some(p) => p,
        None => {
            tracing::error!("PipeWire: invalid pod bytes");
            return;
        }
    };
    let mut params = [pod];

    let ring_ref = ring.clone();

    let _listener = match stream
        .add_local_listener_with_user_data(())
        .process(move |stream, _| {
            if let Some(mut buffer) = stream.dequeue_buffer() {
                let datas = buffer.datas_mut();
                if let Some(data) = datas.first_mut()
                    && let Some(slice) = data.data()
                {
                    let n_frames = slice.len() / (ch * 4);
                    // Safety: PipeWire MAP_BUFFERS guarantees the slice is valid F32-aligned
                    // memory for the negotiated format (F32LE).
                    let dst: &mut [f32] = unsafe {
                        std::slice::from_raw_parts_mut(slice.as_ptr() as *mut f32, n_frames * ch)
                    };
                    ring_ref.read(dst);
                    let chunk = data.chunk_mut();
                    *chunk.size_mut() = (n_frames * ch * 4) as u32;
                    *chunk.stride_mut() = (ch * 4) as i32;
                    *chunk.offset_mut() = 0;
                }
            }
        })
        .register()
    {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("PipeWire listener: {e}");
            return;
        }
    };

    if let Err(e) = stream.connect(
        pw::spa::utils::Direction::Output,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    ) {
        tracing::error!("PipeWire connect: {e}");
        return;
    }

    // Signal readiness to the main thread
    if let Ok(mut ready) = signal.ready.lock() {
        *ready = true;
        signal.condvar.notify_one();
    }

    // Run the main loop, checking the running flag periodically
    let loop_ref = main_loop.loop_();
    while running.load(Ordering::Relaxed) {
        loop_ref.iterate(Duration::from_millis(10));
    }
}
