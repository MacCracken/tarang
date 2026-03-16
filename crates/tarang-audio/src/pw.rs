//! PipeWire audio output backend
//!
//! Streams decoded F32 audio to PipeWire via its Rust bindings.
//! Requires the `pipewire` feature and `libpipewire-0.3` system library.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use pipewire as pw;
use pw::spa::pod::Pod;
use pw::spa::utils::Direction;
use pw::stream::{Stream, StreamFlags};

use tarang_core::{AudioBuffer, Result, TarangError};

use super::{AudioOutput, OutputConfig};

/// Ring buffer shared between the main thread and the PipeWire callback
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
        // Zero-fill remainder
        for i in to_read..dst.len() {
            dst[i] = 0.0;
        }
        to_read
    }
}

/// PipeWire audio output sink
pub struct PipeWireOutput {
    config: Option<OutputConfig>,
    ring: Arc<Mutex<RingBuffer>>,
    main_loop: Option<pw::main_loop::MainLoop>,
    _stream: Option<Stream>,
}

impl PipeWireOutput {
    pub fn new() -> Self {
        Self {
            config: None,
            ring: Arc::new(Mutex::new(RingBuffer::new(0))),
            main_loop: None,
            _stream: None,
        }
    }
}

impl AudioOutput for PipeWireOutput {
    fn open(&mut self, config: &OutputConfig) -> Result<()> {
        pw::init();

        // Ring buffer sized for ~200ms of audio
        let ring_size = (config.sample_rate as usize * config.channels as usize) / 5;
        self.ring = Arc::new(Mutex::new(RingBuffer::new(ring_size.max(8192))));

        let main_loop = pw::main_loop::MainLoop::new(None)
            .map_err(|e| TarangError::Pipeline(format!("PipeWire main loop: {e}")))?;

        let stream = Stream::new(
            &main_loop,
            "tarang-audio",
            pw::properties::properties! {
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_ROLE => "Music",
                *pw::keys::MEDIA_CATEGORY => "Playback",
            },
        )
        .map_err(|e| TarangError::Pipeline(format!("PipeWire stream: {e}")))?;

        // Build the SPA audio format pod
        let channels = config.channels as u32;
        let rate = config.sample_rate;

        let mut audio_info = pw::spa::param::audio::AudioInfoRaw::new();
        audio_info.set_format(pw::spa::param::audio::AudioFormat::F32LE);
        audio_info.set_rate(rate);
        audio_info.set_channels(channels);

        let mut params_buf = vec![0u8; 1024];
        let pod = pw::spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(&mut params_buf),
            &pw::spa::pod::Value::Object(pw::spa::pod::Object {
                type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
                id: pw::spa::param::ParamType::EnumFormat.as_raw(),
                properties: audio_info.into(),
            }),
        )
        .map_err(|e| TarangError::Pipeline(format!("PipeWire pod serialize: {e}")))?
        .0
        .into_inner();

        let pod_ref = unsafe { Pod::from_raw(pod.as_ptr() as *const _) };

        let ring_ref = self.ring.clone();
        let ch = config.channels as usize;

        stream
            .add_local_listener()
            .process(move |stream, _| {
                if let Some(mut buffer) = stream.dequeue_buffer() {
                    let datas = buffer.datas_mut();
                    if let Some(data) = datas.first_mut() {
                        let chunk = data.chunk_mut();
                        let n_frames = chunk.size() as usize / (ch * 4);
                        if let Some(slice) = data.data() {
                            let dst: &mut [f32] = unsafe {
                                std::slice::from_raw_parts_mut(
                                    slice.as_ptr() as *mut f32,
                                    n_frames * ch,
                                )
                            };
                            if let Ok(mut ring) = ring_ref.lock() {
                                ring.read(dst);
                            }
                            chunk.set_size((n_frames * ch * 4) as u32);
                            chunk.set_stride((ch * 4) as i32);
                            chunk.set_offset(0);
                        }
                    }
                }
            })
            .register()
            .map_err(|e| TarangError::Pipeline(format!("PipeWire listener: {e}")))?;

        stream
            .connect(
                Direction::Output,
                None,
                StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
                &mut [pod_ref],
            )
            .map_err(|e| TarangError::Pipeline(format!("PipeWire connect: {e}")))?;

        self.config = Some(config.clone());
        self.main_loop = Some(main_loop);
        // Note: stream is moved into _stream to keep it alive
        self._stream = Some(stream);

        Ok(())
    }

    fn write(&mut self, buf: &AudioBuffer) -> Result<()> {
        if self.config.is_none() {
            return Err(TarangError::Pipeline("output not opened".to_string()));
        }

        let samples = unsafe {
            std::slice::from_raw_parts(buf.data.as_ptr() as *const f32, buf.data.len() / 4)
        };

        // Write to ring buffer, spinning briefly if full
        let mut written = 0;
        while written < samples.len() {
            if let Ok(mut ring) = self.ring.lock() {
                let n = ring.write(&samples[written..]);
                written += n;
            }
            if written < samples.len() {
                std::thread::sleep(Duration::from_micros(100));
            }
        }

        // Pump the main loop to drive callbacks
        if let Some(ref main_loop) = self.main_loop {
            main_loop.iterate(Duration::ZERO);
        }

        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        // Drain the ring buffer by pumping the loop
        if let Some(ref main_loop) = self.main_loop {
            for _ in 0..100 {
                let avail = self.ring.lock().map(|r| r.available()).unwrap_or(0);
                if avail == 0 {
                    break;
                }
                main_loop.iterate(Duration::from_millis(10));
            }
        }
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        self._stream = None;
        self.main_loop = None;
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
