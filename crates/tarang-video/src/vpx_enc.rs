//! VP8/VP9 encoding via libvpx FFI
//!
//! Safe Rust wrapper around libvpx for VP8 and VP9 encoding.
//! Requires the `vpx-enc` feature and libvpx system library.

use tarang_core::{Result, TarangError, VideoCodec, VideoFrame};

/// RAII guard for `vpx_image_t` — ensures `vpx_img_free` is called even on panic.
struct VpxImageGuard {
    img: vpx_sys::vpx_image_t,
}

impl Drop for VpxImageGuard {
    fn drop(&mut self) {
        unsafe { vpx_sys::vpx_img_free(&mut self.img) };
    }
}

/// VP8/VP9 encoder configuration
#[derive(Debug, Clone)]
pub struct VpxEncoderConfig {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub bitrate_bps: u32,
    pub frame_rate_num: u32,
    pub frame_rate_den: u32,
    pub threads: u32,
    pub speed: i32,
}

impl Default for VpxEncoderConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodec::Vp9,
            width: 1920,
            height: 1080,
            bitrate_bps: 4_000_000,
            frame_rate_num: 30,
            frame_rate_den: 1,
            threads: 4,
            speed: 6,
        }
    }
}

/// VP8/VP9 encoder powered by libvpx
pub struct VpxEncoder {
    codec: VideoCodec,
    ctx: vpx_sys::vpx_codec_ctx_t,
    width: u32,
    height: u32,
    frames_encoded: u64,
    pts: i64,
}

// VpxEncoder owns the codec context exclusively; safe to move across threads.
// SAFETY: libvpx codec contexts are not shared — each VpxEncoder has sole ownership.
unsafe impl Send for VpxEncoder {}

impl VpxEncoder {
    pub fn new(config: &VpxEncoderConfig) -> Result<Self> {
        if config.width == 0 || config.height == 0 {
            return Err(TarangError::Pipeline(
                "VpxEncoder: width and height must be non-zero".to_string(),
            ));
        }
        if config.frame_rate_num == 0 || config.frame_rate_den == 0 {
            return Err(TarangError::Pipeline(
                "VpxEncoder: frame_rate_num and frame_rate_den must be non-zero".to_string(),
            ));
        }
        if config.frame_rate_num > i32::MAX as u32 || config.frame_rate_den > i32::MAX as u32 {
            return Err(TarangError::Pipeline(
                "VpxEncoder: frame_rate_num/den must fit in i32".to_string(),
            ));
        }

        let iface = match config.codec {
            VideoCodec::Vp8 => unsafe { vpx_sys::vpx_codec_vp8_cx() },
            VideoCodec::Vp9 => unsafe { vpx_sys::vpx_codec_vp9_cx() },
            other => {
                return Err(TarangError::UnsupportedCodec(format!(
                    "VpxEncoder does not support {other}"
                )));
            }
        };

        // Get default encoder config — use MaybeUninit since the struct may contain
        // types that cannot be zero-initialized (e.g. function pointers)
        let mut cfg = std::mem::MaybeUninit::<vpx_sys::vpx_codec_enc_cfg_t>::uninit();
        let res = unsafe { vpx_sys::vpx_codec_enc_config_default(iface, cfg.as_mut_ptr(), 0) };
        if res != vpx_sys::vpx_codec_err_t::VPX_CODEC_OK {
            return Err(TarangError::Pipeline(format!(
                "vpx_codec_enc_config_default failed: {res:?}"
            )));
        }
        let mut cfg = unsafe { cfg.assume_init() };

        cfg.g_w = config.width;
        cfg.g_h = config.height;
        cfg.rc_target_bitrate = config.bitrate_bps / 1000;
        cfg.g_timebase.num = config.frame_rate_den as i32;
        cfg.g_timebase.den = config.frame_rate_num as i32;
        cfg.g_threads = config.threads;
        cfg.g_error_resilient = 0;

        let mut ctx: vpx_sys::vpx_codec_ctx_t = unsafe { std::mem::zeroed() };
        let res = unsafe {
            vpx_sys::vpx_codec_enc_init_ver(
                &mut ctx,
                iface,
                &cfg,
                0,
                vpx_sys::VPX_ENCODER_ABI_VERSION as i32,
            )
        };

        if res != vpx_sys::vpx_codec_err_t::VPX_CODEC_OK {
            return Err(TarangError::Pipeline(format!(
                "vpx_codec_enc_init failed: {res:?}"
            )));
        }

        // Set speed/quality tradeoff for VP9
        if config.codec == VideoCodec::Vp9 {
            let ctl_res = unsafe {
                vpx_sys::vpx_codec_control_(
                    &mut ctx,
                    vpx_sys::vp8e_enc_control_id::VP8E_SET_CPUUSED as i32,
                    config.speed,
                )
            };
            if ctl_res != vpx_sys::vpx_codec_err_t::VPX_CODEC_OK {
                unsafe { vpx_sys::vpx_codec_destroy(&mut ctx) };
                return Err(TarangError::Pipeline(format!(
                    "vpx VP8E_SET_CPUUSED failed: {ctl_res:?}"
                )));
            }
        }

        Ok(Self {
            codec: config.codec,
            ctx,
            width: config.width,
            height: config.height,
            frames_encoded: 0,
            pts: 0,
        })
    }

    /// Encode a YUV420p frame. Uses the frame's timestamp for PTS.
    /// Returns encoded packets (may be empty if encoder is buffering).
    pub fn encode(&mut self, frame: &VideoFrame) -> Result<Vec<Vec<u8>>> {
        // Compute plane sizes in usize to avoid u32 overflow
        let y_size = self.width as usize * self.height as usize;
        let chroma_w = ((self.width + 1) / 2) as usize;
        let chroma_h = ((self.height + 1) / 2) as usize;
        let expected_size = y_size + 2 * chroma_w * chroma_h;

        if frame.data.len() < expected_size {
            return Err(TarangError::Pipeline(format!(
                "VideoFrame data too small: got {} bytes, expected {expected_size}",
                frame.data.len()
            )));
        }

        let mut raw_img: vpx_sys::vpx_image_t = unsafe { std::mem::zeroed() };
        let alloc_result = unsafe {
            vpx_sys::vpx_img_alloc(
                &mut raw_img,
                vpx_sys::vpx_img_fmt::VPX_IMG_FMT_I420,
                self.width,
                self.height,
                1,
            )
        };

        if alloc_result.is_null() {
            return Err(TarangError::Pipeline("vpx_img_alloc failed".to_string()));
        }

        // RAII guard created only after successful alloc — no forget() needed
        let guard = VpxImageGuard { img: raw_img };

        // Copy YUV420p planes from VideoFrame into vpx_image
        // Use isize arithmetic to correctly handle negative strides
        // Y plane
        for row in 0..self.height as usize {
            let src_start = row * self.width as usize;
            let dst_offset = row as isize * guard.img.stride[0] as isize;
            let dst_ptr = unsafe { guard.img.planes[0].offset(dst_offset) };
            unsafe {
                std::ptr::copy_nonoverlapping(
                    frame.data[src_start..].as_ptr(),
                    dst_ptr,
                    self.width as usize,
                );
            }
        }

        // U plane
        let u_offset = y_size;
        for row in 0..chroma_h {
            let src_start = u_offset + row * chroma_w;
            let dst_offset = row as isize * guard.img.stride[1] as isize;
            let dst_ptr = unsafe { guard.img.planes[1].offset(dst_offset) };
            unsafe {
                std::ptr::copy_nonoverlapping(frame.data[src_start..].as_ptr(), dst_ptr, chroma_w);
            }
        }

        // V plane
        let v_offset = u_offset + chroma_w * chroma_h;
        for row in 0..chroma_h {
            let src_start = v_offset + row * chroma_w;
            let dst_offset = row as isize * guard.img.stride[2] as isize;
            let dst_ptr = unsafe { guard.img.planes[2].offset(dst_offset) };
            unsafe {
                std::ptr::copy_nonoverlapping(frame.data[src_start..].as_ptr(), dst_ptr, chroma_w);
            }
        }

        // Use frame timestamp as PTS (in timebase units)
        let pts = frame.timestamp.as_millis() as i64;

        // VPX_DL_GOOD_QUALITY = 1000000
        let res =
            unsafe { vpx_sys::vpx_codec_encode(&mut self.ctx, &guard.img, pts, 1, 0, 1_000_000) };

        // guard drops here, calling vpx_img_free
        drop(guard);

        if res != vpx_sys::vpx_codec_err_t::VPX_CODEC_OK {
            return Err(TarangError::Pipeline(format!(
                "vpx_codec_encode failed: {res:?}"
            )));
        }

        self.pts = pts + 1;
        let packets = self.drain_packets();
        self.frames_encoded += 1;
        Ok(packets)
    }

    /// Flush the encoder — signal end of stream and drain remaining packets.
    pub fn flush(&mut self) -> Result<Vec<Vec<u8>>> {
        let res = unsafe {
            vpx_sys::vpx_codec_encode(&mut self.ctx, std::ptr::null(), self.pts, 1, 0, 1_000_000)
        };

        if res != vpx_sys::vpx_codec_err_t::VPX_CODEC_OK {
            return Err(TarangError::Pipeline(format!(
                "vpx_codec_encode flush failed: {res:?}"
            )));
        }

        Ok(self.drain_packets())
    }

    fn drain_packets(&mut self) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        let mut iter: vpx_sys::vpx_codec_iter_t = std::ptr::null();

        loop {
            let pkt = unsafe { vpx_sys::vpx_codec_get_cx_data(&mut self.ctx, &mut iter) };
            if pkt.is_null() {
                break;
            }

            let pkt = unsafe { &*pkt };
            if pkt.kind == vpx_sys::vpx_codec_cx_pkt_kind::VPX_CODEC_CX_FRAME_PKT {
                let frame_data = unsafe { pkt.data.frame };
                let buf = unsafe {
                    std::slice::from_raw_parts(frame_data.buf as *const u8, frame_data.sz)
                };
                packets.push(buf.to_vec());
            }
        }

        packets
    }

    pub fn codec(&self) -> VideoCodec {
        self.codec
    }

    pub fn frames_encoded(&self) -> u64 {
        self.frames_encoded
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl Drop for VpxEncoder {
    fn drop(&mut self) {
        unsafe {
            vpx_sys::vpx_codec_destroy(&mut self.ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::time::Duration;
    use tarang_core::PixelFormat;

    fn make_yuv420p_frame(width: u32, height: u32) -> VideoFrame {
        let y_size = (width as usize) * (height as usize);
        let chroma_w = ((width + 1) / 2) as usize;
        let chroma_h = ((height + 1) / 2) as usize;
        let total = y_size + 2 * chroma_w * chroma_h;
        let mut data = vec![128u8; total];
        for i in 0..y_size {
            data[i] = (i % 256) as u8;
        }
        VideoFrame {
            data: Bytes::from(data),
            pixel_format: PixelFormat::Yuv420p,
            width,
            height,
            timestamp: Duration::from_millis(33),
        }
    }

    #[test]
    fn encoder_creation_vp9() {
        let config = VpxEncoderConfig {
            codec: VideoCodec::Vp9,
            width: 320,
            height: 240,
            bitrate_bps: 500_000,
            speed: 9,
            threads: 1,
            ..Default::default()
        };
        let encoder = VpxEncoder::new(&config).unwrap();
        assert_eq!(encoder.codec(), VideoCodec::Vp9);
        assert_eq!(encoder.dimensions(), (320, 240));
        assert_eq!(encoder.frames_encoded(), 0);
    }

    #[test]
    fn encoder_creation_vp8() {
        let config = VpxEncoderConfig {
            codec: VideoCodec::Vp8,
            width: 320,
            height: 240,
            ..Default::default()
        };
        let encoder = VpxEncoder::new(&config).unwrap();
        assert_eq!(encoder.codec(), VideoCodec::Vp8);
    }

    #[test]
    fn encoder_rejects_zero_dimensions() {
        let config = VpxEncoderConfig {
            width: 0,
            height: 240,
            ..Default::default()
        };
        assert!(VpxEncoder::new(&config).is_err());
    }

    #[test]
    fn encoder_rejects_zero_framerate() {
        let config = VpxEncoderConfig {
            frame_rate_num: 0,
            ..Default::default()
        };
        assert!(VpxEncoder::new(&config).is_err());
    }

    #[test]
    fn encoder_rejects_unsupported_codec() {
        let config = VpxEncoderConfig {
            codec: VideoCodec::H264,
            ..Default::default()
        };
        assert!(VpxEncoder::new(&config).is_err());
    }

    #[test]
    fn encode_single_frame_vp8() {
        let config = VpxEncoderConfig {
            codec: VideoCodec::Vp8,
            width: 320,
            height: 240,
            bitrate_bps: 500_000,
            threads: 1,
            ..Default::default()
        };
        let mut encoder = VpxEncoder::new(&config).unwrap();
        let frame = make_yuv420p_frame(320, 240);
        let packets = encoder.encode(&frame).unwrap();
        assert!(
            !packets.is_empty(),
            "VP8 encoder should produce output for first frame"
        );
        assert_eq!(encoder.frames_encoded(), 1);
    }

    #[test]
    fn encode_and_flush_vp9() {
        let config = VpxEncoderConfig {
            codec: VideoCodec::Vp9,
            width: 160,
            height: 120,
            bitrate_bps: 200_000,
            speed: 9,
            threads: 1,
            ..Default::default()
        };
        let mut encoder = VpxEncoder::new(&config).unwrap();

        let mut total_packets = 0;
        for i in 0..3 {
            let mut frame = make_yuv420p_frame(160, 120);
            frame.timestamp = Duration::from_millis(i * 33);
            let packets = encoder.encode(&frame).unwrap();
            total_packets += packets.len();
        }

        let flushed = encoder.flush().unwrap();
        total_packets += flushed.len();

        assert!(
            total_packets > 0,
            "should produce packets after encode + flush"
        );
        assert_eq!(encoder.frames_encoded(), 3);
    }

    #[test]
    fn encode_rejects_short_data() {
        let config = VpxEncoderConfig {
            width: 320,
            height: 240,
            ..Default::default()
        };
        let mut encoder = VpxEncoder::new(&config).unwrap();
        let frame = VideoFrame {
            data: Bytes::from(vec![0u8; 100]),
            pixel_format: PixelFormat::Yuv420p,
            width: 320,
            height: 240,
            timestamp: Duration::ZERO,
        };
        assert!(encoder.encode(&frame).is_err());
    }
}
