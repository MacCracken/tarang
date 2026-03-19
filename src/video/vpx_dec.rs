//! VP8/VP9 decoding via libvpx FFI
//!
//! Safe Rust wrapper around libvpx for VP8 and VP9 decoding.
//! Requires the `vpx` feature and libvpx system library.

use bytes::Bytes;
use std::time::Duration;
use crate::core::{PixelFormat, Result, TarangError, VideoCodec, VideoFrame};

/// VP8/VP9 decoder powered by libvpx
pub struct VpxDecoder {
    codec: VideoCodec,
    ctx: vpx_sys::vpx_codec_ctx_t,
    frames_decoded: u64,
}

// VpxDecoder owns the codec context exclusively; safe to move across threads.
// SAFETY: libvpx codec contexts are not shared — each VpxDecoder has sole ownership.
unsafe impl Send for VpxDecoder {}

impl VpxDecoder {
    pub fn new(codec: VideoCodec) -> Result<Self> {
        // Safety: vpx_codec_vp8_dx/vp9_dx return static function pointers; always valid.
        let iface = match codec {
            VideoCodec::Vp8 => unsafe { vpx_sys::vpx_codec_vp8_dx() },
            VideoCodec::Vp9 => unsafe { vpx_sys::vpx_codec_vp9_dx() },
            other => {
                return Err(TarangError::UnsupportedCodec(format!(
                    "VpxDecoder does not support {other}"
                )));
            }
        };

        // Safety: vpx_codec_ctx_t is a C struct with no invariants; zero-init is valid pre-init state.
        let mut ctx: vpx_sys::vpx_codec_ctx_t = unsafe { std::mem::zeroed() };
        let cfg: *const vpx_sys::vpx_codec_dec_cfg_t = std::ptr::null();

        let res = unsafe {
            vpx_sys::vpx_codec_dec_init_ver(
                &mut ctx,
                iface,
                cfg,
                0,
                vpx_sys::VPX_DECODER_ABI_VERSION as i32,
            )
        };

        if res != vpx_sys::vpx_codec_err_t::VPX_CODEC_OK {
            return Err(TarangError::DecodeError(format!(
                "vpx_codec_dec_init failed: {res:?}"
            )));
        }

        Ok(Self {
            codec,
            ctx,
            frames_decoded: 0,
        })
    }

    /// Decode a VP8/VP9 packet. Returns decoded frames (may be 0 or 1).
    pub fn decode(&mut self, data: &[u8], timestamp: Duration) -> Result<Vec<VideoFrame>> {
        if data.len() > u32::MAX as usize {
            return Err(TarangError::DecodeError(
                "packet exceeds u32::MAX bytes".to_string(),
            ));
        }

        let res = unsafe {
            vpx_sys::vpx_codec_decode(
                &mut self.ctx,
                data.as_ptr(),
                data.len() as u32,
                std::ptr::null_mut(),
                0,
            )
        };

        if res != vpx_sys::vpx_codec_err_t::VPX_CODEC_OK {
            return Err(TarangError::DecodeError(format!(
                "vpx_codec_decode failed: {res:?}"
            )));
        }

        let mut frames = Vec::new();
        let mut iter: vpx_sys::vpx_codec_iter_t = std::ptr::null();

        loop {
            let img = unsafe { vpx_sys::vpx_codec_get_frame(&mut self.ctx, &mut iter) };
            if img.is_null() {
                break;
            }

            let img = unsafe { &*img };
            let width = img.d_w;
            let height = img.d_h;

            if width == 0 || height == 0 {
                continue;
            }

            // Verify I420 format — VP9 profile 1+ can produce 4:2:2/4:4:4
            if img.fmt != vpx_sys::vpx_img_fmt::VPX_IMG_FMT_I420 {
                return Err(TarangError::DecodeError(format!(
                    "unsupported pixel format from libvpx: {:?}, expected I420",
                    img.fmt
                )));
            }

            // Pre-allocate output buffer
            let chroma_w = width.div_ceil(2) as usize;
            let chroma_h = height.div_ceil(2) as usize;
            let y_size = width as usize * height as usize;
            let mut yuv_data = Vec::with_capacity(y_size + 2 * chroma_w * chroma_h);

            // Copy YUV420p planes using isize stride arithmetic (handles negative strides).
            // Safety: libvpx guarantees planes[0..3] point to valid I420 image data with
            // stride[0..3] bytes per row. Format was validated above. Pointers remain valid
            // until the next vpx_codec_decode call, which we don't make within this scope.
            // Y plane
            for row in 0..height as isize {
                let offset = row * img.stride[0] as isize;
                let ptr = unsafe { img.planes[0].offset(offset) };
                let slice = unsafe { std::slice::from_raw_parts(ptr, width as usize) };
                yuv_data.extend_from_slice(slice);
            }

            // U plane
            for row in 0..chroma_h as isize {
                let offset = row * img.stride[1] as isize;
                let ptr = unsafe { img.planes[1].offset(offset) };
                let slice = unsafe { std::slice::from_raw_parts(ptr, chroma_w) };
                yuv_data.extend_from_slice(slice);
            }

            // V plane
            for row in 0..chroma_h as isize {
                let offset = row * img.stride[2] as isize;
                let ptr = unsafe { img.planes[2].offset(offset) };
                let slice = unsafe { std::slice::from_raw_parts(ptr, chroma_w) };
                yuv_data.extend_from_slice(slice);
            }

            self.frames_decoded += 1;

            frames.push(VideoFrame {
                data: Bytes::from(yuv_data),
                pixel_format: PixelFormat::Yuv420p,
                width,
                height,
                timestamp,
            });
        }

        Ok(frames)
    }

    pub fn codec(&self) -> VideoCodec {
        self.codec
    }

    pub fn frames_decoded(&self) -> u64 {
        self.frames_decoded
    }
}

impl Drop for VpxDecoder {
    fn drop(&mut self) {
        unsafe {
            vpx_sys::vpx_codec_destroy(&mut self.ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_creation_vp8() {
        let decoder = VpxDecoder::new(VideoCodec::Vp8).unwrap();
        assert_eq!(decoder.codec(), VideoCodec::Vp8);
        assert_eq!(decoder.frames_decoded(), 0);
    }

    #[test]
    fn decoder_creation_vp9() {
        let decoder = VpxDecoder::new(VideoCodec::Vp9).unwrap();
        assert_eq!(decoder.codec(), VideoCodec::Vp9);
    }

    #[test]
    fn decoder_rejects_unsupported_codec() {
        assert!(VpxDecoder::new(VideoCodec::H264).is_err());
        assert!(VpxDecoder::new(VideoCodec::Av1).is_err());
    }

    #[test]
    fn decode_invalid_data_returns_error() {
        let mut decoder = VpxDecoder::new(VideoCodec::Vp8).unwrap();
        let result = decoder.decode(&[0xDE, 0xAD, 0xBE, 0xEF], Duration::ZERO);
        match result {
            Err(_) => {}
            Ok(frames) => assert!(frames.is_empty(), "invalid data should not produce frames"),
        }
    }

    #[test]
    fn vp8_encode_decode_roundtrip() {
        use crate::video::vpx_enc::{VpxEncoder, VpxEncoderConfig};

        let enc_config = VpxEncoderConfig {
            codec: VideoCodec::Vp8,
            width: 320,
            height: 240,
            bitrate_bps: 500_000,
            threads: 1,
            ..Default::default()
        };
        let mut encoder = VpxEncoder::new(&enc_config).unwrap();

        let y_size = 320 * 240;
        let chroma = 160 * 120;
        let mut data = vec![128u8; y_size + 2 * chroma];
        for i in 0..y_size {
            data[i] = (i % 256) as u8;
        }

        let frame = crate::core::VideoFrame {
            data: bytes::Bytes::from(data),
            pixel_format: crate::core::PixelFormat::Yuv420p,
            width: 320,
            height: 240,
            timestamp: Duration::ZERO,
        };

        let mut encoded_packets = Vec::new();
        for i in 0..3 {
            let mut f = frame.clone();
            f.timestamp = Duration::from_millis(i * 33);
            let packets = encoder.encode(&f).unwrap();
            encoded_packets.extend(packets);
        }
        let flushed = encoder.flush().unwrap();
        encoded_packets.extend(flushed);
        assert!(!encoded_packets.is_empty());

        let mut decoder = VpxDecoder::new(VideoCodec::Vp8).unwrap();
        let mut decoded_count = 0;
        for packet in &encoded_packets {
            let frames = decoder.decode(packet, Duration::ZERO).unwrap();
            for f in &frames {
                assert_eq!(f.width, 320);
                assert_eq!(f.height, 240);
                assert_eq!(f.pixel_format, crate::core::PixelFormat::Yuv420p);
                decoded_count += 1;
            }
        }
        assert!(decoded_count > 0, "should decode at least one frame");
    }
}
