//! VA-API hardware-accelerated encoding
//!
//! Wraps the VA-API encode pipeline for GPU-accelerated H.264/HEVC encoding.
//! Requires the `vaapi` feature and a GPU with VA-API encode support.
//!
//! This module provides surface-level encode orchestration using cros-libva.
//! The VA-API driver handles the actual codec bitstream generation (SPS/PPS,
//! slice headers, rate control) — we manage surfaces, buffers, and the
//! encode→sync→readback lifecycle.
//!
//! # Example
//!
//! ```rust,ignore
//! use tarang::video::vaapi_enc::{VaapiEncoder, VaapiEncoderConfig};
//!
//! let config = VaapiEncoderConfig { width: 1920, height: 1080, ..Default::default() };
//! let mut encoder = VaapiEncoder::new(&config).unwrap();
//! let encoded = encoder.encode(&yuv_frame).unwrap();
//! ```

use crate::core::{Result, TarangError, VideoCodec, VideoFrame};
use cros_libva::{
    BufferType,
    Config,
    Display,
    EncPictureParameter,
    // H.264 parameter types (re-exported from buffer::h264)
    EncPictureParameterBufferH264,
    EncSequenceParameter,
    EncSequenceParameterBufferH264,
    EncSliceParameter,
    EncSliceParameterBufferH264,
    H264EncPicFields,
    H264EncSeqFields,
    H264VuiFields,
    Image,
    MappedCodedBuffer,
    Picture,
    PictureH264,
    Surface,
    UsageHint,
    VA_INVALID_ID,
    VA_INVALID_SURFACE,
    // Misc
    VAConfigAttrib,
    VAConfigAttribType,
    VAEntrypoint,
    VAProfile,
};
use std::rc::Rc;

/// VA-API encoder configuration
#[derive(Debug, Clone)]
pub struct VaapiEncoderConfig {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub bitrate_bps: u32,
    pub frame_rate_num: u32,
    pub frame_rate_den: u32,
    /// DRM render node path override (default: auto-detect)
    pub device: Option<String>,
}

impl Default for VaapiEncoderConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodec::H264,
            width: 1920,
            height: 1080,
            bitrate_bps: 5_000_000,
            frame_rate_num: 30,
            frame_rate_den: 1,
            device: None,
        }
    }
}

/// Map a VideoCodec to the best VA-API profile for encoding.
fn codec_to_va_profile(codec: VideoCodec) -> Result<VAProfile::Type> {
    match codec {
        VideoCodec::H264 => Ok(VAProfile::VAProfileH264Main),
        VideoCodec::H265 => Ok(VAProfile::VAProfileHEVCMain),
        _ => Err(TarangError::UnsupportedCodec(
            format!("VA-API encoding not supported for {codec}").into(),
        )),
    }
}

use super::vaapi_common::{open_display, va_err};

/// Find the best encode entrypoint for a profile on a display.
fn find_encode_entrypoint(
    display: &Display,
    profile: VAProfile::Type,
) -> Result<VAEntrypoint::Type> {
    let entrypoints = display.query_config_entrypoints(profile).map_err(|e| {
        TarangError::HwAccelError(format!("failed to query entrypoints: {e:?}").into())
    })?;

    // Prefer low-power (fixed-function) encoder, fall back to standard
    if entrypoints.contains(&VAEntrypoint::VAEntrypointEncSliceLP) {
        Ok(VAEntrypoint::VAEntrypointEncSliceLP)
    } else if entrypoints.contains(&VAEntrypoint::VAEntrypointEncSlice) {
        Ok(VAEntrypoint::VAEntrypointEncSlice)
    } else {
        Err(TarangError::HwAccelError(
            format!("no encode entrypoint for VA profile {profile}").into(),
        ))
    }
}

/// Convert YUV420p frame data to NV12 layout for VA-API surface upload.
///
/// YUV420p: Y plane, U plane (w/2 * h/2), V plane (w/2 * h/2)
/// NV12:    Y plane, interleaved UV plane (w * h/2)
fn yuv420p_to_nv12(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let y_size = w * h;
    let chroma_w = w.div_ceil(2);
    let chroma_h = h.div_ceil(2);
    let chroma_count = chroma_w * chroma_h;

    let mut nv12 = vec![0u8; y_size + chroma_count * 2];

    // Copy Y plane as-is
    nv12[..y_size].copy_from_slice(&data[..y_size]);

    // Interleave U and V into NV12 UV plane
    let u_plane = &data[y_size..y_size + chroma_count];
    let v_plane = &data[y_size + chroma_count..];
    let uv_dst = &mut nv12[y_size..];
    for i in 0..chroma_count {
        uv_dst[i * 2] = u_plane[i];
        uv_dst[i * 2 + 1] = v_plane[i];
    }

    nv12
}

/// Create invalid PictureH264 reference (for unused reference frame slots).
fn invalid_pic_h264() -> PictureH264 {
    PictureH264::new(VA_INVALID_ID, 0, VA_INVALID_SURFACE, 0, 0)
}

/// Hardware-accelerated video encoder using VA-API.
///
/// Supports H.264 encoding via the GPU's fixed-function or shader-based
/// encode hardware. The VA-API driver handles bitstream generation — this
/// wrapper manages the surface lifecycle.
pub struct VaapiEncoder {
    display: Rc<Display>,
    _config: Config,
    context: Rc<cros_libva::Context>,
    codec: VideoCodec,
    width: u32,
    height: u32,
    bitrate_bps: u32,
    frame_rate_num: u32,
    frame_rate_den: u32,
    frames_encoded: u64,
    /// Cached NV12 image format (queried once in constructor).
    nv12_fmt: cros_libva::VAImageFormat,
    /// Pre-allocated surface pool for encode (avoids per-frame GPU allocation).
    surface_pool: Vec<Surface<()>>,
}

/// Number of surfaces to rotate through for encoding.
const NUM_SURFACES: usize = 4;

impl VaapiEncoder {
    /// Create a new VA-API hardware encoder.
    ///
    /// This verifies that the requested codec is supported for encoding
    /// on the available GPU hardware and initializes the full encode pipeline.
    pub fn new(config: &VaapiEncoderConfig) -> Result<Self> {
        crate::core::validate_video_dimensions(config.width, config.height)?;

        if config.codec != VideoCodec::H264 {
            return Err(TarangError::HwAccelError(
                format!(
                    "VA-API encode currently implements H.264 only, got {}",
                    config.codec
                )
                .into(),
            ));
        }

        let profile = codec_to_va_profile(config.codec)?;
        let display = open_display(&config.device)?;
        let entrypoint = find_encode_entrypoint(&display, profile)?;

        // Query supported RT format
        let mut attrs = vec![VAConfigAttrib {
            type_: VAConfigAttribType::VAConfigAttribRTFormat,
            value: 0,
        }];
        display
            .get_config_attributes(profile, entrypoint, &mut attrs)
            .map_err(|e| {
                TarangError::HwAccelError(format!("failed to get config attributes: {e:?}").into())
            })?;

        let va_config = display
            .create_config(attrs, profile, entrypoint)
            .map_err(|e| {
                TarangError::HwAccelError(format!("failed to create VA config: {e:?}").into())
            })?;

        // Create surfaces for encoding (NV12 format, encoder usage hint)
        let surfaces = display
            .create_surfaces(
                cros_libva::VA_RT_FORMAT_YUV420,
                None,
                config.width,
                config.height,
                Some(UsageHint::USAGE_HINT_ENCODER),
                vec![(); NUM_SURFACES],
            )
            .map_err(|e| {
                TarangError::HwAccelError(format!("failed to create surfaces: {e:?}").into())
            })?;

        let context = display
            .create_context(
                &va_config,
                config.width,
                config.height,
                Some(&surfaces),
                true,
            )
            .map_err(|e| {
                TarangError::HwAccelError(format!("failed to create context: {e:?}").into())
            })?;

        // Context holds surface IDs internally; pool surfaces for per-frame reuse
        let surface_pool = surfaces;

        // Cache NV12 image format (avoid per-frame ioctl)
        let image_fmts = display.query_image_formats().map_err(|e| {
            TarangError::HwAccelError(format!("failed to query image formats: {e:?}").into())
        })?;
        let nv12_fmt = image_fmts
            .into_iter()
            .find(|f| f.fourcc == cros_libva::VA_FOURCC_NV12)
            .ok_or_else(|| {
                TarangError::HwAccelError("driver does not support NV12 image format".into())
            })?;

        Ok(Self {
            display,
            _config: va_config,
            context,
            codec: config.codec,
            width: config.width,
            height: config.height,
            bitrate_bps: config.bitrate_bps,
            frame_rate_num: config.frame_rate_num,
            frame_rate_den: config.frame_rate_den,
            frames_encoded: 0,
            nv12_fmt,
            surface_pool,
        })
    }

    /// Check if the encoder was successfully initialized with hardware support.
    pub fn is_hardware_accelerated(&self) -> bool {
        true // If new() succeeded, we have HW support
    }

    /// Encode a YUV420p frame using VA-API hardware.
    ///
    /// Returns the encoded H.264 bitstream for this frame. Currently encodes
    /// all frames as IDR (intra) frames for simplicity — inter-frame prediction
    /// (P/B frames) will be added in a future release.
    pub fn encode(&mut self, frame: &VideoFrame) -> Result<Vec<u8>> {
        if frame.width != self.width || frame.height != self.height {
            return Err(TarangError::HwAccelError(
                format!(
                    "frame dimensions {}x{} don't match encoder {}x{}",
                    frame.width, frame.height, self.width, self.height
                )
                .into(),
            ));
        }

        let nv12_data = yuv420p_to_nv12(&frame.data, self.width, self.height);

        let nv12_fmt = self.nv12_fmt;

        // Create coded buffer for output bitstream
        let coded_buffer = self
            .context
            .create_enc_coded(nv12_data.len())
            .map_err(|e| {
                TarangError::HwAccelError(format!("failed to create coded buffer: {e:?}").into())
            })?;

        let w = self.width as usize;
        let h = self.height as usize;
        let w_mbs = self.width.div_ceil(16) as u16;
        let h_mbs = self.height.div_ceil(16) as u16;
        let time_scale = self.frame_rate_num * 2;

        // Build H.264 SPS (Sequence Parameter Set)
        let seq_fields = H264EncSeqFields::new(
            1, // chroma_format_idc = 4:2:0
            1, // frame_mbs_only_flag = progressive only
            0, // mb_adaptive_frame_field_flag
            0, // seq_scaling_matrix_present_flag
            0, // direct_8x8_inference_flag
            1, // log2_max_frame_num_minus4
            0, // pic_order_cnt_type
            2, // log2_max_pic_order_cnt_lsb_minus4
            0, // delta_pic_order_always_zero_flag
        );

        let sps_buf = BufferType::EncSequenceParameter(EncSequenceParameter::H264(
            EncSequenceParameterBufferH264::new(
                0,                // seq_parameter_set_id
                41,               // level_idc (4.1 — supports 1080p30)
                30,               // intra_period
                30,               // intra_idr_period
                1,                // ip_period
                self.bitrate_bps, // bits_per_second
                1,                // max_num_ref_frames
                w_mbs,            // picture_width_in_mbs
                h_mbs,            // picture_height_in_mbs
                &seq_fields,
                0,        // bit_depth_luma_minus8
                0,        // bit_depth_chroma_minus8
                0,        // num_ref_frames_in_pic_order_cnt_cycle
                0,        // offset_for_non_ref_pic
                0,        // offset_for_top_to_bottom_field
                [0; 256], // offset_for_ref_frame
                None,     // frame_crop
                Some(H264VuiFields::new(1, 1, 0, 0, 0, 1, 0, 0)),
                255,                 // aspect_ratio_idc (extended SAR)
                1,                   // sar_width
                1,                   // sar_height
                self.frame_rate_den, // num_units_in_tick
                time_scale,          // time_scale
            ),
        ));

        let sps = self.context.create_buffer(sps_buf).map_err(|e| {
            TarangError::HwAccelError(format!("failed to create SPS buffer: {e:?}").into())
        })?;

        let frame_num = (self.frames_encoded % 65536) as u16;
        let pic_fields = H264EncPicFields::new(
            1, // idr_pic_flag (all IDR for now)
            1, // reference_pic_flag
            0, // entropy_coding_mode_flag (CAVLC)
            0, // weighted_pred_flag
            0, // weighted_bipred_idc
            0, // constrained_intra_pred_flag
            0, // transform_8x8_mode_flag
            1, // deblocking_filter_control_present_flag
            0, // redundant_pic_cnt_present_flag
            0, // pic_order_present_flag
            0, // pic_scaling_matrix_present_flag
        );

        // Build slice parameter
        let ref_list_0: [PictureH264; 32] = std::array::from_fn(|_| invalid_pic_h264());
        let ref_list_1: [PictureH264; 32] = std::array::from_fn(|_| invalid_pic_h264());

        let num_mbs = w_mbs as u32 * h_mbs as u32;
        let slice_buf = BufferType::EncSliceParameter(EncSliceParameter::H264(
            EncSliceParameterBufferH264::new(
                0,             // macroblock_address
                num_mbs,       // num_macroblocks
                VA_INVALID_ID, // macroblock_info
                2,             // slice_type = I
                0,             // pic_parameter_set_id
                1,             // idr_pic_id
                0,             // pic_order_cnt_lsb
                0,             // delta_pic_order_cnt_bottom
                [0, 0],        // delta_pic_order_cnt
                1,             // direct_spatial_mv_pred_flag
                0,             // num_ref_idx_active_override_flag
                0,             // num_ref_idx_l0_active_minus1
                0,             // num_ref_idx_l1_active_minus1
                ref_list_0,
                ref_list_1,
                0,            // luma_log2_weight_denom
                0,            // chroma_log2_weight_denom
                0,            // luma_weight_l0_flag
                [0; 32],      // luma_weight_l0
                [0; 32],      // luma_offset_l0
                0,            // chroma_weight_l0_flag
                [[0; 2]; 32], // chroma_weight_l0
                [[0; 2]; 32], // chroma_offset_l0
                0,            // luma_weight_l1_flag
                [0; 32],      // luma_weight_l1
                [0; 32],      // luma_offset_l1
                0,            // chroma_weight_l1_flag
                [[0; 2]; 32], // chroma_weight_l1
                [[0; 2]; 32], // chroma_offset_l1
                0,            // cabac_init_idc
                0,            // slice_qp_delta
                0,            // disable_deblocking_filter_idc
                2,            // slice_alpha_c0_offset_div2
                2,            // slice_beta_offset_div2
            ),
        ));

        let slice = self.context.create_buffer(slice_buf).map_err(|e| {
            TarangError::HwAccelError(format!("failed to create slice buffer: {e:?}").into())
        })?;

        // Reuse a surface from the pool, or allocate a new one if empty
        let enc_surface = if let Some(s) = self.surface_pool.pop() {
            s
        } else {
            let mut enc_surfaces = self
                .display
                .create_surfaces(
                    cros_libva::VA_RT_FORMAT_YUV420,
                    None,
                    self.width,
                    self.height,
                    Some(UsageHint::USAGE_HINT_ENCODER),
                    vec![()],
                )
                .map_err(|e| {
                    TarangError::HwAccelError(
                        format!("failed to create encode surface: {e:?}").into(),
                    )
                })?;
            enc_surfaces.pop().ok_or_else(|| {
                TarangError::HwAccelError("VA-API returned no encode surfaces".into())
            })?
        };

        // Upload NV12 to the new surface
        let mut enc_image = Image::create_from(
            &enc_surface,
            nv12_fmt,
            (self.width, self.height),
            (self.width, self.height),
        )
        .map_err(|e| {
            TarangError::HwAccelError(format!("failed to create encode image: {e:?}").into())
        })?;

        let enc_va_image = *enc_image.image();
        let enc_dest = enc_image.as_mut();

        // Copy luma
        let mut s_off = 0;
        let mut d_off = enc_va_image.offsets[0] as usize;
        for _ in 0..h {
            enc_dest[d_off..d_off + w].copy_from_slice(&nv12_data[s_off..s_off + w]);
            d_off += enc_va_image.pitches[0] as usize;
            s_off += w;
        }
        // Copy chroma
        s_off = w * h;
        d_off = enc_va_image.offsets[1] as usize;
        for _ in 0..h / 2 {
            enc_dest[d_off..d_off + w].copy_from_slice(&nv12_data[s_off..s_off + w]);
            d_off += enc_va_image.pitches[1] as usize;
            s_off += w;
        }
        drop(enc_image);

        // Rebuild PPS with the new surface ID
        let enc_surface_id = enc_surface.id();
        let ref_frames2: [PictureH264; 16] = std::array::from_fn(|_| invalid_pic_h264());
        let pps_buf2 = BufferType::EncPictureParameter(EncPictureParameter::H264(
            EncPictureParameterBufferH264::new(
                PictureH264::new(enc_surface_id, 0, 0, 0, 0),
                ref_frames2,
                coded_buffer.id(),
                0,
                0,
                0,
                frame_num,
                26,
                0,
                0,
                0,
                0,
                &pic_fields,
            ),
        ));
        let pps2 = self.context.create_buffer(pps_buf2).map_err(|e| {
            TarangError::HwAccelError(format!("failed to create PPS buffer: {e:?}").into())
        })?;

        let mut picture = Picture::new(self.frames_encoded, Rc::clone(&self.context), enc_surface);
        picture.add_buffer(sps);
        picture.add_buffer(pps2);
        picture.add_buffer(slice);

        let picture = picture.begin().map_err(|e| {
            TarangError::HwAccelError(format!("vaBeginPicture failed: {e:?}").into())
        })?;
        let picture = picture.render().map_err(|e| {
            TarangError::HwAccelError(format!("vaRenderPicture failed: {e:?}").into())
        })?;
        let picture = picture
            .end()
            .map_err(|e| TarangError::HwAccelError(format!("vaEndPicture failed: {e:?}").into()))?;
        let picture = picture.sync().map_err(|(e, _)| {
            TarangError::HwAccelError(format!("vaSyncSurface failed: {e:?}").into())
        })?;

        // Reclaim surface for reuse
        if let Ok(surface) = picture.take_surface() {
            self.surface_pool.push(surface);
        }

        // Read back encoded bitstream
        let mapped = MappedCodedBuffer::new(&coded_buffer).map_err(|e| {
            TarangError::HwAccelError(format!("failed to map coded buffer: {e:?}").into())
        })?;

        let mut bitstream = Vec::new();
        for segment in mapped.iter() {
            bitstream.extend_from_slice(segment.buf);
        }

        if bitstream.is_empty() {
            return Err(TarangError::HwAccelError(
                "VA-API encode produced empty bitstream".into(),
            ));
        }

        self.frames_encoded += 1;
        Ok(bitstream)
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
    fn codec_to_profile_h264() {
        assert_eq!(
            codec_to_va_profile(VideoCodec::H264).unwrap(),
            VAProfile::VAProfileH264Main
        );
    }

    #[test]
    fn codec_to_profile_hevc() {
        assert_eq!(
            codec_to_va_profile(VideoCodec::H265).unwrap(),
            VAProfile::VAProfileHEVCMain
        );
    }

    #[test]
    fn codec_to_profile_unsupported() {
        assert!(codec_to_va_profile(VideoCodec::Vp8).is_err());
        assert!(codec_to_va_profile(VideoCodec::Av1).is_err());
    }

    #[test]
    fn config_default() {
        let config = VaapiEncoderConfig::default();
        assert_eq!(config.codec, VideoCodec::H264);
        assert_eq!(config.width, 1920);
        assert_eq!(config.height, 1080);
    }

    #[test]
    fn rejects_zero_dimensions() {
        let config = VaapiEncoderConfig {
            width: 0,
            height: 480,
            ..Default::default()
        };
        assert!(VaapiEncoder::new(&config).is_err());
    }

    #[test]
    fn rejects_odd_dimensions() {
        let config = VaapiEncoderConfig {
            width: 321,
            height: 240,
            ..Default::default()
        };
        assert!(VaapiEncoder::new(&config).is_err());
    }

    #[test]
    fn rejects_hevc_codec() {
        let config = VaapiEncoderConfig {
            codec: VideoCodec::H265,
            ..Default::default()
        };
        let result = VaapiEncoder::new(&config);
        assert!(result.is_err());
    }

    #[test]
    fn yuv420p_to_nv12_basic() {
        // 4x2 frame: Y=4*2=8, U=2*1=2, V=2*1=2 = 12 bytes YUV420p
        // NV12: Y=8, UV=4 = 12 bytes
        let yuv = vec![
            // Y plane (4x2)
            10, 20, 30, 40, 50, 60, 70, 80, // U plane (2x1)
            100, 110, // V plane (2x1)
            200, 210,
        ];
        let nv12 = yuv420p_to_nv12(&yuv, 4, 2);
        assert_eq!(nv12.len(), 12);
        // Y should be identical
        assert_eq!(&nv12[..8], &yuv[..8]);
        // UV should be interleaved: U0,V0, U1,V1
        assert_eq!(nv12[8], 100); // U0
        assert_eq!(nv12[9], 200); // V0
        assert_eq!(nv12[10], 110); // U1
        assert_eq!(nv12[11], 210); // V1
    }

    #[test]
    #[ignore] // Requires VA-API hardware with H.264 encode support
    fn encoder_creation_h264() {
        let config = VaapiEncoderConfig {
            codec: VideoCodec::H264,
            width: 320,
            height: 240,
            ..Default::default()
        };
        let encoder = VaapiEncoder::new(&config).unwrap();
        assert!(encoder.is_hardware_accelerated());
        assert_eq!(encoder.codec(), VideoCodec::H264);
        assert_eq!(encoder.dimensions(), (320, 240));
        assert!(!encoder.driver_name().is_empty());
        println!("VA-API encoder driver: {}", encoder.driver_name());
    }

    #[test]
    #[ignore] // Requires VA-API hardware with H.264 encode support
    fn encode_single_frame() {
        let w = 320u32;
        let h = 240u32;
        let config = VaapiEncoderConfig {
            codec: VideoCodec::H264,
            width: w,
            height: h,
            bitrate_bps: 1_000_000,
            ..Default::default()
        };
        let mut encoder = VaapiEncoder::new(&config).unwrap();

        // Create a black YUV420p frame
        let y_size = (w * h) as usize;
        let uv_size = y_size / 4;
        let mut data = vec![16u8; y_size]; // Y = 16 (black in TV range)
        data.extend(vec![128u8; uv_size]); // U = 128
        data.extend(vec![128u8; uv_size]); // V = 128

        let frame = VideoFrame {
            data: bytes::Bytes::from(data),
            pixel_format: crate::core::PixelFormat::Yuv420p,
            width: w,
            height: h,
            timestamp: std::time::Duration::ZERO,
        };

        let bitstream = encoder.encode(&frame).unwrap();
        assert!(
            !bitstream.is_empty(),
            "encoded bitstream should not be empty"
        );
        assert_eq!(encoder.frames_encoded(), 1);
        println!("Encoded frame: {} bytes", bitstream.len());
    }

    #[test]
    fn yuv420p_to_nv12_16x16() {
        let w = 16u32;
        let h = 16u32;
        let y_size = (w * h) as usize;
        let uv_size = y_size / 4;
        let mut yuv = vec![128u8; y_size]; // Y
        yuv.extend(vec![64u8; uv_size]); // U
        yuv.extend(vec![192u8; uv_size]); // V

        let nv12 = yuv420p_to_nv12(&yuv, w, h);
        assert_eq!(nv12.len(), y_size + y_size / 2);
        // Y plane unchanged
        assert!(nv12[..y_size].iter().all(|&b| b == 128));
        // UV plane: interleaved U=64, V=192
        for i in 0..uv_size {
            assert_eq!(nv12[y_size + i * 2], 64, "U at {i}");
            assert_eq!(nv12[y_size + i * 2 + 1], 192, "V at {i}");
        }
    }

    #[test]
    fn yuv420p_to_nv12_2x2() {
        // Smallest valid YUV420p: 2x2
        let yuv = vec![
            1, 2, 3, 4,  // Y (2x2)
            10, // U (1x1)
            20, // V (1x1)
        ];
        let nv12 = yuv420p_to_nv12(&yuv, 2, 2);
        assert_eq!(nv12.len(), 6); // Y=4 + UV=2
        assert_eq!(&nv12[..4], &[1, 2, 3, 4]);
        assert_eq!(nv12[4], 10); // U
        assert_eq!(nv12[5], 20); // V
    }
}
