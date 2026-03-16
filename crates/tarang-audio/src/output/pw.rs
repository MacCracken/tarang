//! PipeWire audio output backend
//!
//! Streams decoded F32 audio to PipeWire via its Rust bindings.
//! Requires the `pipewire` feature and `libpipewire-0.3` system library.
//!
//! Architecture: the PipeWire main loop runs on a dedicated thread. The caller
//! writes F32 samples into a shared ring buffer. The PipeWire process callback
//! reads from the ring buffer to fill output buffers.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tarang_core::{AudioBuffer, Result, TarangError};

use super::{AudioOutput, OutputConfig};

/// Ring buffer shared between the caller and the PipeWire callback thread
struct RingBuffer {
    data: Vec<f32>,
    read_pos: usize,
    write_pos: usize,
    capacity: usize,
}

impl RingBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            data: vec![0.0; capacity],
            read_pos: 0,
            write_pos: 0,
            capacity,
        }
    }

    fn available(&self) -> usize {
        if self.write_pos >= self.read_pos {
            self.write_pos - self.read_pos
        } else {
            self.capacity - self.read_pos + self.write_pos
        }
    }

    fn free_space(&self) -> usize {
        self.capacity - 1 - self.available()
    }

    fn write(&mut self, samples: &[f32]) -> usize {
        let to_write = samples.len().min(self.free_space());
        for i in 0..to_write {
            self.data[self.write_pos] = samples[i];
            self.write_pos = (self.write_pos + 1) % self.capacity;
        }
        to_write
    }

    fn read(&mut self, dst: &mut [f32]) -> usize {
        let to_read = dst.len().min(self.available());
        for i in 0..to_read {
            dst[i] = self.data[self.read_pos];
            self.read_pos = (self.read_pos + 1) % self.capacity;
        }
        for i in to_read..dst.len() {
            dst[i] = 0.0;
        }
        to_read
    }
}

/// PipeWire audio output sink.
///
/// The PipeWire main loop runs on a background thread. Audio samples are
/// written into a ring buffer and consumed by the PipeWire process callback.
pub struct PipeWireOutput {
    config: Option<OutputConfig>,
    ring: Arc<Mutex<RingBuffer>>,
    running: Arc<AtomicBool>,
    pw_thread: Option<std::thread::JoinHandle<()>>,
}

impl PipeWireOutput {
    pub fn new() -> Self {
        Self {
            config: None,
            ring: Arc::new(Mutex::new(RingBuffer::new(0))),
            running: Arc::new(AtomicBool::new(false)),
            pw_thread: None,
        }
    }
}

impl AudioOutput for PipeWireOutput {
    fn open(&mut self, config: &OutputConfig) -> Result<()> {
        // Ring buffer sized for ~500ms of audio
        let ring_size = (config.sample_rate as usize * config.channels as usize) / 2;
        self.ring = Arc::new(Mutex::new(RingBuffer::new(ring_size.max(16384))));
        self.running = Arc::new(AtomicBool::new(true));

        let ring_ref = self.ring.clone();
        let running_ref = self.running.clone();
        let rate = config.sample_rate;
        let channels = config.channels as u32;
        let ch = config.channels as usize;

        let handle = std::thread::Builder::new()
            .name("tarang-pipewire".into())
            .spawn(move || {
                pw_thread_main(ring_ref, running_ref, rate, channels, ch);
            })
            .map_err(|e| TarangError::Pipeline(format!("spawn PipeWire thread: {e}")))?;

        self.pw_thread = Some(handle);
        self.config = Some(config.clone());

        // Give PipeWire a moment to initialize
        std::thread::sleep(Duration::from_millis(50));

        Ok(())
    }

    fn write(&mut self, buf: &AudioBuffer) -> Result<()> {
        if self.config.is_none() {
            return Err(TarangError::Pipeline("output not opened".to_string()));
        }

        let byte_len = buf.data.len();
        if byte_len % 4 != 0 {
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
            if let Ok(mut ring) = self.ring.lock() {
                let n = ring.write(&samples[written..]);
                written += n;
            }
            if written < samples.len() {
                std::thread::sleep(Duration::from_micros(200));
            }
        }

        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        for _ in 0..500 {
            let avail = self.ring.lock().map(|r| r.available()).unwrap_or(0);
            if avail == 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
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
        let avail = self.ring.lock().map(|r| r.available()).unwrap_or(0);
        if let Some(ref config) = self.config {
            if config.sample_rate > 0 && config.channels > 0 {
                let frames = avail / config.channels as usize;
                return Duration::from_secs_f64(frames as f64 / config.sample_rate as f64);
            }
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
    ring: Arc<Mutex<RingBuffer>>,
    running: Arc<AtomicBool>,
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
                if let Some(data) = datas.first_mut() {
                    if let Some(slice) = data.data() {
                        let n_frames = slice.len() / (ch * 4);
                        let dst: &mut [f32] = unsafe {
                            std::slice::from_raw_parts_mut(
                                slice.as_ptr() as *mut f32,
                                n_frames * ch,
                            )
                        };
                        if let Ok(mut r) = ring_ref.lock() {
                            r.read(dst);
                        }
                        let chunk = data.chunk_mut();
                        *chunk.size_mut() = (n_frames * ch * 4) as u32;
                        *chunk.stride_mut() = (ch * 4) as i32;
                        *chunk.offset_mut() = 0;
                    }
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

    // Run the main loop, checking the running flag periodically
    let loop_ref = main_loop.loop_();
    while running.load(Ordering::Relaxed) {
        loop_ref.iterate(Duration::from_millis(10));
    }
}
