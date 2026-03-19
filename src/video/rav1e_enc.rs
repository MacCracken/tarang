//! AV1 encoding via rav1e (pure Rust)
//!
//! Safe wrapper around rav1e for AV1 encoding.
//! Requires the `rav1e` feature.

use crate::core::{Result, TarangError, VideoFrame};

/// AV1 encoder configuration
#[derive(Debug, Clone)]
pub struct Rav1eConfig {
    pub width: u32,
    pub height: u32,
    pub frame_rate_num: u32,
    pub frame_rate_den: u32,
    pub bitrate_bps: u32,
    pub speed: u32,
}

impl Default for Rav1eConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            frame_rate_num: 30,
            frame_rate_den: 1,
            bitrate_bps: 5_000_000,
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
        enc_config.speed_settings =
            rav1e::config::SpeedSettings::from_preset(config.speed.min(255) as u8);
        enc_config.bitrate = (config.bitrate_bps).min(i32::MAX as u32) as i32;
        enc_config.time_base = rav1e::data::Rational {
            num: config.frame_rate_den as u64,
            den: config.frame_rate_num as u64,
        };

        let rav1e_cfg = rav1e::Config::new()
            .with_encoder_config(enc_config)
            .with_threads(0); // auto

        let context = rav1e_cfg
            .new_context()
            .map_err(|e| TarangError::Pipeline(format!("rav1e context creation failed: {e}")))?;

        if config.width % 2 != 0 || config.height % 2 != 0 {
            return Err(TarangError::Pipeline(format!(
                "rav1e requires even dimensions, got {}x{}",
                config.width, config.height
            )));
        }

        Ok(Self {
            context,
            frames_encoded: 0,
            width: config.width,
            height: config.height,
        })
    }

    /// Send a YUV420p frame to the encoder.
    pub fn send_frame(&mut self, frame: &VideoFrame) -> Result<()> {
        if frame.width != self.width || frame.height != self.height {
            return Err(TarangError::Pipeline(format!(
                "frame dimensions {}x{} don't match encoder {}x{}",
                frame.width, frame.height, self.width, self.height
            )));
        }

        let mut enc_frame = self.context.new_frame();

        let y_size = (self.width * self.height) as usize;
        let chroma_w = (self.width / 2) as usize;
        let chroma_h = (self.height / 2) as usize;
        let expected_size = crate::core::yuv420p_frame_size(self.width, self.height);
        if frame.data.len() < expected_size {
            return Err(TarangError::Pipeline(format!(
                "frame data too small: {} bytes, expected at least {}",
                frame.data.len(),
                expected_size
            )));
        }

        // Y plane
        {
            let stride = enc_frame.planes[0].cfg.stride;
            let plane = enc_frame.planes[0].data_origin_mut();
            let needed = (self.height as usize - 1) * stride + self.width as usize;
            if plane.len() < needed {
                return Err(TarangError::Pipeline(format!(
                    "rav1e Y plane buffer too small: {} < {needed}",
                    plane.len()
                )));
            }
            for row in 0..self.height as usize {
                let src_start = row * self.width as usize;
                let src_end = src_start + self.width as usize;
                let dst_start = row * stride;
                plane[dst_start..dst_start + self.width as usize]
                    .copy_from_slice(&frame.data[src_start..src_end]);
            }
        }

        // U plane
        {
            let u_offset = y_size;
            let stride = enc_frame.planes[1].cfg.stride;
            let plane = enc_frame.planes[1].data_origin_mut();
            let needed = (chroma_h - 1) * stride + chroma_w;
            if plane.len() < needed {
                return Err(TarangError::Pipeline(format!(
                    "rav1e U plane buffer too small: {} < {needed}",
                    plane.len()
                )));
            }
            for row in 0..chroma_h {
                let src_start = u_offset + row * chroma_w;
                let src_end = src_start + chroma_w;
                let dst_start = row * stride;
                plane[dst_start..dst_start + chroma_w]
                    .copy_from_slice(&frame.data[src_start..src_end]);
            }
        }

        // V plane
        {
            let v_offset = y_size + chroma_w * chroma_h;
            let stride = enc_frame.planes[2].cfg.stride;
            let plane = enc_frame.planes[2].data_origin_mut();
            let needed = (chroma_h - 1) * stride + chroma_w;
            if plane.len() < needed {
                return Err(TarangError::Pipeline(format!(
                    "rav1e V plane buffer too small: {} < {needed}",
                    plane.len()
                )));
            }
            for row in 0..chroma_h {
                let src_start = v_offset + row * chroma_w;
                let src_end = src_start + chroma_w;
                let dst_start = row * stride;
                plane[dst_start..dst_start + chroma_w]
                    .copy_from_slice(&frame.data[src_start..src_end]);
            }
        }

        self.context
            .send_frame(enc_frame)
            .map_err(|e| TarangError::Pipeline(format!("rav1e send_frame: {e}")))?;

        Ok(())
    }

    /// Receive encoded AV1 packets. Returns None if encoder needs more data.
    pub fn receive_packet(&mut self) -> Result<Option<Vec<u8>>> {
        match self.context.receive_packet() {
            Ok(packet) => {
                self.frames_encoded += 1;
                Ok(Some(packet.data))
            }
            Err(rav1e::EncoderStatus::NeedMoreData) => Ok(None),
            Err(rav1e::EncoderStatus::LimitReached) => Ok(None),
            Err(e) => Err(TarangError::Pipeline(format!("rav1e receive_packet: {e}"))),
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
                Err(rav1e::EncoderStatus::Encoded) => continue,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::PixelFormat;
    use bytes::Bytes;
    use std::time::Duration;

    fn make_yuv420p_frame(width: u32, height: u32) -> VideoFrame {
        let y_size = (width * height) as usize;
        let chroma_w = (width / 2) as usize;
        let chroma_h = (height / 2) as usize;
        let total = y_size + 2 * chroma_w * chroma_h;
        let mut data = vec![0u8; total];
        // Gradient Y plane
        for i in 0..y_size {
            data[i] = (i % 256) as u8;
        }
        // Flat chroma
        for i in y_size..total {
            data[i] = 128;
        }
        VideoFrame {
            data: Bytes::from(data),
            pixel_format: PixelFormat::Yuv420p,
            width,
            height,
            timestamp: Duration::ZERO,
        }
    }

    #[test]
    fn encoder_creation() {
        let config = Rav1eConfig {
            width: 320,
            height: 240,
            speed: 10,
            ..Default::default()
        };
        let encoder = Rav1eEncoder::new(&config).unwrap();
        assert_eq!(encoder.frames_encoded(), 0);
    }

    #[test]
    fn encoder_rejects_odd_dimensions() {
        let config = Rav1eConfig {
            width: 321,
            height: 240,
            ..Default::default()
        };
        assert!(Rav1eEncoder::new(&config).is_err());
    }

    #[test]
    fn encoder_rejects_odd_height() {
        let config = Rav1eConfig {
            width: 320,
            height: 241,
            ..Default::default()
        };
        assert!(Rav1eEncoder::new(&config).is_err());
    }

    #[test]
    fn send_frame_dimension_mismatch() {
        let config = Rav1eConfig {
            width: 320,
            height: 240,
            speed: 10,
            ..Default::default()
        };
        let mut encoder = Rav1eEncoder::new(&config).unwrap();
        let frame = make_yuv420p_frame(640, 480);
        assert!(encoder.send_frame(&frame).is_err());
    }

    #[test]
    fn send_frame_data_too_small() {
        let config = Rav1eConfig {
            width: 320,
            height: 240,
            speed: 10,
            ..Default::default()
        };
        let mut encoder = Rav1eEncoder::new(&config).unwrap();
        let frame = VideoFrame {
            data: Bytes::from(vec![0u8; 100]),
            pixel_format: PixelFormat::Yuv420p,
            width: 320,
            height: 240,
            timestamp: Duration::ZERO,
        };
        assert!(encoder.send_frame(&frame).is_err());
    }

    #[test]
    fn encode_single_frame() {
        let config = Rav1eConfig {
            width: 64,
            height: 64,
            speed: 10,
            bitrate_bps: 100_000,
            ..Default::default()
        };
        let mut encoder = Rav1eEncoder::new(&config).unwrap();
        let frame = make_yuv420p_frame(64, 64);
        encoder.send_frame(&frame).unwrap();
        // May or may not produce a packet (encoder may buffer)
        let _ = encoder.receive_packet().unwrap();
    }

    #[test]
    fn encode_and_flush() {
        let config = Rav1eConfig {
            width: 64,
            height: 64,
            speed: 10,
            bitrate_bps: 100_000,
            ..Default::default()
        };
        let mut encoder = Rav1eEncoder::new(&config).unwrap();

        let mut total_packets = 0;
        for i in 0..3 {
            let mut frame = make_yuv420p_frame(64, 64);
            frame.timestamp = Duration::from_millis(i * 33);
            encoder.send_frame(&frame).unwrap();
            // Drain all available packets before sending more
            while let Ok(Some(_)) = encoder.receive_packet() {
                total_packets += 1;
            }
        }

        let flushed = encoder.flush().unwrap();
        total_packets += flushed.len();
        assert!(
            total_packets > 0,
            "should produce at least one packet after flush"
        );
    }

    #[test]
    fn flush_empty_encoder() {
        let config = Rav1eConfig {
            width: 64,
            height: 64,
            speed: 10,
            ..Default::default()
        };
        let mut encoder = Rav1eEncoder::new(&config).unwrap();
        let flushed = encoder.flush().unwrap();
        assert!(flushed.is_empty());
    }

    #[test]
    fn receive_without_send_returns_none() {
        let config = Rav1eConfig {
            width: 64,
            height: 64,
            speed: 10,
            ..Default::default()
        };
        let mut encoder = Rav1eEncoder::new(&config).unwrap();
        let result = encoder.receive_packet().unwrap();
        assert!(result.is_none());
    }
}
