//! AV1 encoding via rav1e (pure Rust)
//!
//! Safe wrapper around rav1e for AV1 encoding.
//! Requires the `rav1e` feature.

use bytes::Bytes;
use tarang_core::{Result, TarangError, VideoFrame};

/// AV1 encoder configuration
#[derive(Debug, Clone)]
pub struct Rav1eConfig {
    pub width: u32,
    pub height: u32,
    pub frame_rate_num: u64,
    pub frame_rate_den: u64,
    pub bitrate: u32,
    pub speed: usize,
}

impl Default for Rav1eConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            frame_rate_num: 30,
            frame_rate_den: 1,
            bitrate: 5_000_000,
            speed: 6,
        }
    }
}

/// AV1 encoder powered by rav1e
pub struct Rav1eEncoder {
    context: rav1e::Context<u8>,
    frames_encoded: u64,
    width: u32,
    height: u32,
}

impl Rav1eEncoder {
    pub fn new(config: &Rav1eConfig) -> Result<Self> {
        let mut enc_config = rav1e::EncoderConfig::default();
        enc_config.width = config.width as usize;
        enc_config.height = config.height as usize;
        enc_config.speed_settings = rav1e::SpeedSettings::from_preset(config.speed);
        enc_config.bitrate = config.bitrate as i32;
        enc_config.time_base = rav1e::data::Rational {
            num: config.frame_rate_den,
            den: config.frame_rate_num,
        };

        let rav1e_cfg = rav1e::Config::new()
            .with_encoder_config(enc_config)
            .with_threads(0); // auto

        let context = rav1e_cfg
            .new_context()
            .map_err(|e| TarangError::Pipeline(format!("rav1e context creation failed: {e}")))?;

        Ok(Self {
            context,
            frames_encoded: 0,
            width: config.width,
            height: config.height,
        })
    }

    /// Send a YUV420p frame to the encoder.
    pub fn send_frame(&mut self, frame: &VideoFrame) -> Result<()> {
        let mut enc_frame = self
            .context
            .new_frame();

        // Copy Y plane
        let y_size = (self.width * self.height) as usize;
        let chroma_w = (self.width / 2) as usize;
        let chroma_h = (self.height / 2) as usize;

        // Y
        for row in 0..self.height as usize {
            let src_start = row * self.width as usize;
            let src_end = src_start + self.width as usize;
            let dst = &mut enc_frame.planes[0].data_origin_mut()
                [row * enc_frame.planes[0].cfg.stride..];
            dst[..self.width as usize].copy_from_slice(&frame.data[src_start..src_end]);
        }

        // U
        let u_offset = y_size;
        for row in 0..chroma_h {
            let src_start = u_offset + row * chroma_w;
            let src_end = src_start + chroma_w;
            let dst = &mut enc_frame.planes[1].data_origin_mut()
                [row * enc_frame.planes[1].cfg.stride..];
            dst[..chroma_w].copy_from_slice(&frame.data[src_start..src_end]);
        }

        // V
        let v_offset = u_offset + chroma_w * chroma_h;
        for row in 0..chroma_h {
            let src_start = v_offset + row * chroma_w;
            let src_end = src_start + chroma_w;
            let dst = &mut enc_frame.planes[2].data_origin_mut()
                [row * enc_frame.planes[2].cfg.stride..];
            dst[..chroma_w].copy_from_slice(&frame.data[src_start..src_end]);
        }

        self.context
            .send_frame(enc_frame)
            .map_err(|e| TarangError::Pipeline(format!("rav1e send_frame: {e}")))?;

        Ok(())
    }

    /// Receive encoded AV1 packets. Returns empty vec if encoder needs more data.
    pub fn receive_packet(&mut self) -> Result<Option<Vec<u8>>> {
        match self.context.receive_packet() {
            Ok(packet) => {
                self.frames_encoded += 1;
                Ok(Some(packet.data))
            }
            Err(rav1e::EncoderStatus::NeedMoreData) => Ok(None),
            Err(rav1e::EncoderStatus::LimitReached) => Ok(None),
            Err(e) => Err(TarangError::Pipeline(format!(
                "rav1e receive_packet: {e}"
            ))),
        }
    }

    /// Signal end of stream and flush remaining packets.
    pub fn flush(&mut self) -> Result<Vec<Vec<u8>>> {
        self.context.flush();

        let mut packets = Vec::new();
        loop {
            match self.context.receive_packet() {
                Ok(packet) => {
                    self.frames_encoded += 1;
                    packets.push(packet.data);
                }
                Err(rav1e::EncoderStatus::LimitReached) => break,
                Err(rav1e::EncoderStatus::NeedMoreData) => break,
                Err(rav1e::EncoderStatus::EnoughData) => continue,
                Err(e) => {
                    return Err(TarangError::Pipeline(format!("rav1e flush: {e}")));
                }
            }
        }
        Ok(packets)
    }

    pub fn frames_encoded(&self) -> u64 {
        self.frames_encoded
    }
}
