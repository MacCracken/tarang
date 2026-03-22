//! VA-API hardware-accelerated video decoding.
//!
//! Requires the `vaapi` feature and a GPU with VA-API decode support.
//! Use [`VaapiDecoder::new`] to create a decoder for a given codec,
//! then feed compressed packets via [`decode`](VaapiDecoder::decode).
//!
//! # Example
//!
//! ```rust,ignore
//! use tarang::video::vaapi_dec::VaapiDecoder;
//! use tarang::core::VideoCodec;
//!
//! let mut decoder = VaapiDecoder::new(VideoCodec::H264, 1920, 1080, None).unwrap();
//! let frame = decoder.decode(&h264_packet, timestamp).unwrap();
//! ```

use crate::core::{PixelFormat, Result, TarangError, VideoCodec, VideoFrame};
use bytes::Bytes;
use cros_libva::{
    BufferType, Config, Display, Picture, Surface, UsageHint, VA_FOURCC_NV12, VA_RT_FORMAT_YUV420,
    VAConfigAttrib, VAConfigAttribType, VAEntrypoint, VAProfile,
};
use std::rc::Rc;
use std::time::Duration;

/// Number of decode surfaces to rotate through.
const NUM_SURFACES: usize = 8;

/// Map a VideoCodec to the VA-API profile for decoding.
fn codec_to_decode_profile(codec: VideoCodec) -> Result<VAProfile::Type> {
    match codec {
        VideoCodec::H264 => Ok(VAProfile::VAProfileH264High),
        VideoCodec::H265 => Ok(VAProfile::VAProfileHEVCMain),
        VideoCodec::Vp9 => Ok(VAProfile::VAProfileVP9Profile0),
        VideoCodec::Av1 => Ok(VAProfile::VAProfileAV1Profile0),
        VideoCodec::Vp8 => Ok(VAProfile::VAProfileVP8Version0_3),
        _ => Err(TarangError::UnsupportedCodec(
            format!("VA-API decode not supported for {codec}").into(),
        )),
    }
}

use super::vaapi_common::{open_display, va_err};

/// VA-API hardware-accelerated video decoder.
///
/// Decodes compressed video packets using GPU hardware. Outputs NV12
/// frames which are converted to YUV420p for compatibility with the
/// rest of the pipeline.
pub struct VaapiDecoder {
    display: Rc<Display>,
    _config: Config,
    context: Rc<cros_libva::Context>,
    codec: VideoCodec,
    width: u32,
    height: u32,
    frames_decoded: u64,
    /// Cached NV12 image format.
    nv12_fmt: cros_libva::VAImageFormat,
    /// Pre-allocated surface pool for decode (avoids per-frame GPU allocation).
    surface_pool: Vec<Surface<()>>,
}

impl VaapiDecoder {
    /// Create a new VA-API decoder for the given codec and dimensions.
    ///
    /// Verifies hardware support and initializes the decode pipeline.
    pub fn new(codec: VideoCodec, width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(TarangError::ConfigError(
                "decoder dimensions must be non-zero".into(),
            ));
        }

        let profile = codec_to_decode_profile(codec)?;
        let display = open_display(&None)?;
        let entrypoint = VAEntrypoint::VAEntrypointVLD;

        // Verify decode support
        let entrypoints = va_err(
            display.query_config_entrypoints(profile),
            "failed to query entrypoints",
        )?;
        if !entrypoints.contains(&entrypoint) {
            return Err(TarangError::HwAccelError(
                format!("VA-API VLD decode not supported for {codec}").into(),
            ));
        }

        let mut attrs = vec![VAConfigAttrib {
            type_: VAConfigAttribType::VAConfigAttribRTFormat,
            value: 0,
        }];
        va_err(
            display.get_config_attributes(profile, entrypoint, &mut attrs),
            "failed to get config attributes",
        )?;

        let config = va_err(
            display.create_config(attrs, profile, entrypoint),
            "failed to create VA config",
        )?;

        // Align height to 16-pixel boundary (required by some drivers)
        let aligned_height = height.div_ceil(16) * 16;

        let surfaces: Vec<Surface<()>> = va_err(
            display.create_surfaces(
                VA_RT_FORMAT_YUV420,
                None,
                width,
                aligned_height,
                Some(UsageHint::USAGE_HINT_DECODER),
                vec![(); NUM_SURFACES],
            ),
            "failed to create surfaces",
        )?;

        let context = va_err(
            display.create_context(&config, width, aligned_height, Some(&surfaces), true),
            "failed to create context",
        )?;

        // Context holds surface IDs internally; pool surfaces for per-frame reuse
        let surface_pool = surfaces;

        // Cache NV12 image format
        let image_fmts = va_err(display.query_image_formats(), "query image formats")?;
        let nv12_fmt = image_fmts
            .into_iter()
            .find(|f| f.fourcc == VA_FOURCC_NV12)
            .ok_or_else(|| TarangError::HwAccelError("NV12 image format not available".into()))?;

        Ok(Self {
            display,
            _config: config,
            context,
            codec,
            width,
            height,
            frames_decoded: 0,
            nv12_fmt,
            surface_pool,
        })
    }

    /// Decode a compressed packet, returning a YUV420p frame if available.
    ///
    /// Submits the compressed data as a SliceData buffer to VA-API.
    /// The driver handles bitstream parsing in long-slice mode.
    ///
    /// Note: full H.264/HEVC decode requires PictureParameter and
    /// SliceParameter buffers constructed from parsed NAL headers.
    /// This implementation passes data as SliceData only — some
    /// drivers may reject this. For reliable decode, use the software
    /// decoders (openh264, dav1d) and reserve VA-API for encode.
    pub fn decode(&mut self, data: &[u8], timestamp: Duration) -> Result<Option<VideoFrame>> {
        if data.is_empty() {
            return Ok(None);
        }

        // Reuse a surface from the pool, or allocate a new one if empty
        let surface = if let Some(s) = self.surface_pool.pop() {
            s
        } else {
            let mut surfaces = self
                .display
                .create_surfaces(
                    VA_RT_FORMAT_YUV420,
                    None,
                    self.width,
                    self.height.div_ceil(16) * 16,
                    Some(UsageHint::USAGE_HINT_DECODER),
                    vec![()],
                )
                .map_err(|e| {
                    TarangError::HwAccelError(
                        format!("failed to create decode surface: {e:?}").into(),
                    )
                })?;
            surfaces.pop().ok_or_else(|| {
                TarangError::HwAccelError("VA-API returned no decode surfaces".into())
            })?
        };

        // Submit slice data
        let slice_data = BufferType::SliceData(data.to_vec());
        let slice_buf = va_err(
            self.context.create_buffer(slice_data),
            "failed to create slice buffer",
        )?;

        let mut picture = Picture::new(self.frames_decoded, Rc::clone(&self.context), surface);
        picture.add_buffer(slice_buf);

        // Submit decode
        let picture = va_err(picture.begin(), "vaBeginPicture failed")?;
        let picture = va_err(picture.render(), "vaRenderPicture failed")?;
        let picture = va_err(picture.end(), "vaEndPicture failed")?;
        let picture = picture.sync().map_err(|(e, _)| {
            TarangError::HwAccelError(format!("vaSyncSurface failed: {e:?}").into())
        })?;

        // Read back decoded frame as NV12
        let nv12_fmt = self.nv12_fmt;

        let image = va_err(
            picture.create_image(
                nv12_fmt,
                (self.width, self.height),
                (self.width, self.height),
            ),
            "create image failed",
        )?;

        // Convert NV12 to YUV420p (read back before reclaiming surface)
        let va_image = *image.image();
        let src = image.as_ref();
        let w = self.width as usize;
        let h = self.height as usize;
        let y_size = w * h;
        let chroma_w = w / 2;
        let chroma_h = h / 2;
        let uv_size = chroma_w * chroma_h;

        let mut yuv420p = Vec::with_capacity(y_size + 2 * uv_size);

        // Copy Y plane
        for row in 0..h {
            let start = va_image.offsets[0] as usize + row * va_image.pitches[0] as usize;
            yuv420p.extend_from_slice(&src[start..start + w]);
        }

        // Deinterleave NV12 UV to separate U and V planes
        let mut u_plane = Vec::with_capacity(uv_size);
        let mut v_plane = Vec::with_capacity(uv_size);
        for row in 0..chroma_h {
            let start = va_image.offsets[1] as usize + row * va_image.pitches[1] as usize;
            for col in 0..chroma_w {
                u_plane.push(src[start + col * 2]);
                v_plane.push(src[start + col * 2 + 1]);
            }
        }
        yuv420p.extend_from_slice(&u_plane);
        yuv420p.extend_from_slice(&v_plane);

        // Drop image to release borrow on picture, then reclaim surface for reuse
        drop(image);
        if let Ok(surface) = picture.take_surface() {
            self.surface_pool.push(surface);
        }

        self.frames_decoded += 1;

        Ok(Some(VideoFrame {
            data: Bytes::from(yuv420p),
            pixel_format: PixelFormat::Yuv420p,
            width: self.width,
            height: self.height,
            timestamp,
        }))
    }

    /// Codec being decoded.
    pub fn codec(&self) -> VideoCodec {
        self.codec
    }

    /// Total frames decoded.
    pub fn frames_decoded(&self) -> u64 {
        self.frames_decoded
    }

    /// Driver name.
    pub fn driver_name(&self) -> String {
        self.display
            .query_vendor_string()
            .unwrap_or_else(|_| "unknown".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_profile_mapping() {
        assert!(codec_to_decode_profile(VideoCodec::H264).is_ok());
        assert!(codec_to_decode_profile(VideoCodec::H265).is_ok());
        assert!(codec_to_decode_profile(VideoCodec::Vp9).is_ok());
        assert!(codec_to_decode_profile(VideoCodec::Av1).is_ok());
        assert!(codec_to_decode_profile(VideoCodec::Vp8).is_ok());
        assert!(codec_to_decode_profile(VideoCodec::Theora).is_err());
    }

    #[test]
    fn rejects_zero_dimensions() {
        assert!(VaapiDecoder::new(VideoCodec::H264, 0, 480).is_err());
        assert!(VaapiDecoder::new(VideoCodec::H264, 640, 0).is_err());
    }

    #[test]
    #[ignore] // Requires VA-API hardware
    fn decoder_creation_h264() {
        let dec = VaapiDecoder::new(VideoCodec::H264, 1920, 1080).unwrap();
        assert_eq!(dec.codec(), VideoCodec::H264);
        assert_eq!(dec.frames_decoded(), 0);
        assert!(!dec.driver_name().is_empty());
        println!("VA-API decoder driver: {}", dec.driver_name());
    }
}
