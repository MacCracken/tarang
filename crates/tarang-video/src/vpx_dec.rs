//! VP8/VP9 decoding via libvpx FFI
//!
//! Safe Rust wrapper around libvpx for VP8 and VP9 decoding.
//! Requires the `vpx` feature and libvpx system library.

use bytes::Bytes;
use std::time::Duration;
use tarang_core::{PixelFormat, Result, TarangError, VideoCodec, VideoFrame};

const DECODER_ABI_VERSION: i32 = {
    macro_rules! parse_i32 {
        () => {
            match i32::from_str_radix(env!("VPX_DECODER_ABI_VERSION"), 10) {
                Ok(v) => v,
                Err(_) => panic!("invalid VPX_DECODER_ABI_VERSION"),
            }
        };
    }
    parse_i32!()
};

/// VP8/VP9 decoder powered by libvpx
pub struct VpxDecoder {
    codec: VideoCodec,
    ctx: vpx_sys::vpx_codec_ctx_t,
    frames_decoded: u64,
    initialized: bool,
}

impl VpxDecoder {
    pub fn new(codec: VideoCodec) -> Result<Self> {
        let iface = match codec {
            VideoCodec::Vp8 => unsafe { vpx_sys::vpx_codec_vp8_dx() },
            VideoCodec::Vp9 => unsafe { vpx_sys::vpx_codec_vp9_dx() },
            other => {
                return Err(TarangError::UnsupportedCodec(format!(
                    "VpxDecoder does not support {other}"
                )));
            }
        };

        let mut ctx: vpx_sys::vpx_codec_ctx_t = unsafe { std::mem::zeroed() };
        let cfg: *const vpx_sys::vpx_codec_dec_cfg_t = std::ptr::null();

        let res = unsafe {
            vpx_sys::vpx_codec_dec_init_ver(
                &mut ctx,
                iface,
                cfg,
                0,
                DECODER_ABI_VERSION,
            )
        };

        if res != vpx_sys::VPX_CODEC_OK {
            return Err(TarangError::DecodeError(format!(
                "vpx_codec_dec_init failed: {res}"
            )));
        }

        Ok(Self {
            codec,
            ctx,
            frames_decoded: 0,
            initialized: true,
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

        if res != vpx_sys::VPX_CODEC_OK {
            return Err(TarangError::DecodeError(format!(
                "vpx_codec_decode failed: {res}"
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
            if img.fmt != vpx_sys::VPX_IMG_FMT_I420 {
                return Err(TarangError::DecodeError(format!(
                    "unsupported pixel format from libvpx: {}, expected I420",
                    img.fmt
                )));
            }

            // Pre-allocate output buffer
            let chroma_w = ((width + 1) / 2) as usize;
            let chroma_h = ((height + 1) / 2) as usize;
            let y_size = width as usize * height as usize;
            let mut yuv_data = Vec::with_capacity(y_size + 2 * chroma_w * chroma_h);

            // Copy YUV420p planes using isize stride arithmetic (handles negative strides)
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

    pub fn frames_decoded(&self) -> u64 {
        self.frames_decoded
    }
}

impl Drop for VpxDecoder {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                vpx_sys::vpx_codec_destroy(&mut self.ctx);
            }
        }
    }
}
