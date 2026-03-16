//! VA-API hardware acceleration detection
//!
//! Probes the system for VA-API support and reports available
//! hardware-accelerated codecs for both decoding and encoding.
//! Requires the `vaapi` feature and libva system library.
//!
//! VDPAU is not supported — Mesa removed VDPAU from all open-source
//! drivers; VA-API is the standard for Linux hardware video acceleration.

use cros_libva::{Display, VAEntrypoint, VAProfile};
use std::path::Path;
use tarang_core::VideoCodec;

/// Direction a hardware codec operates in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwCodecDirection {
    Decode,
    Encode,
}

impl std::fmt::Display for HwCodecDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode => write!(f, "decode"),
            Self::Encode => write!(f, "encode"),
        }
    }
}

/// Describes one hardware codec capability.
#[derive(Debug, Clone)]
pub struct HwCodecCapability {
    pub codec: VideoCodec,
    pub direction: HwCodecDirection,
    pub profile: String,
}

/// Summary of VA-API hardware acceleration on this system.
#[derive(Debug, Clone)]
pub struct HwAccelReport {
    pub driver_name: String,
    pub render_node: String,
    pub capabilities: Vec<HwCodecCapability>,
}

impl HwAccelReport {
    /// Check if a given codec can be hardware-decoded.
    pub fn can_decode(&self, codec: VideoCodec) -> bool {
        self.capabilities
            .iter()
            .any(|c| c.codec == codec && c.direction == HwCodecDirection::Decode)
    }

    /// Check if a given codec can be hardware-encoded.
    pub fn can_encode(&self, codec: VideoCodec) -> bool {
        self.capabilities
            .iter()
            .any(|c| c.codec == codec && c.direction == HwCodecDirection::Encode)
    }

    /// List all codecs that can be hardware-decoded.
    pub fn decode_codecs(&self) -> Vec<VideoCodec> {
        let mut codecs: Vec<VideoCodec> = self
            .capabilities
            .iter()
            .filter(|c| c.direction == HwCodecDirection::Decode)
            .map(|c| c.codec)
            .collect();
        codecs.dedup();
        codecs
    }

    /// List all codecs that can be hardware-encoded.
    pub fn encode_codecs(&self) -> Vec<VideoCodec> {
        let mut codecs: Vec<VideoCodec> = self
            .capabilities
            .iter()
            .filter(|c| c.direction == HwCodecDirection::Encode)
            .map(|c| c.codec)
            .collect();
        codecs.dedup();
        codecs
    }
}

/// Map a VA-API profile to a tarang VideoCodec.
fn profile_to_codec(profile: VAProfile::Type) -> Option<VideoCodec> {
    match profile {
        VAProfile::VAProfileH264Baseline
        | VAProfile::VAProfileH264Main
        | VAProfile::VAProfileH264High
        | VAProfile::VAProfileH264ConstrainedBaseline => Some(VideoCodec::H264),

        VAProfile::VAProfileHEVCMain | VAProfile::VAProfileHEVCMain10 => Some(VideoCodec::H265),

        VAProfile::VAProfileVP8Version0_3 => Some(VideoCodec::Vp8),

        VAProfile::VAProfileVP9Profile0 | VAProfile::VAProfileVP9Profile2 => Some(VideoCodec::Vp9),

        VAProfile::VAProfileAV1Profile0 => Some(VideoCodec::Av1),

        _ => None,
    }
}

/// Map a VA-API profile to a human-readable name.
fn profile_name(profile: VAProfile::Type) -> &'static str {
    match profile {
        VAProfile::VAProfileH264Baseline => "H264Baseline",
        VAProfile::VAProfileH264Main => "H264Main",
        VAProfile::VAProfileH264High => "H264High",
        VAProfile::VAProfileH264ConstrainedBaseline => "H264ConstrainedBaseline",
        VAProfile::VAProfileHEVCMain => "HEVCMain",
        VAProfile::VAProfileHEVCMain10 => "HEVCMain10",
        VAProfile::VAProfileVP8Version0_3 => "VP8",
        VAProfile::VAProfileVP9Profile0 => "VP9Profile0",
        VAProfile::VAProfileVP9Profile2 => "VP9Profile2",
        VAProfile::VAProfileAV1Profile0 => "AV1Profile0",
        _ => "Unknown",
    }
}

/// Map a VA-API entrypoint to a codec direction.
fn entrypoint_direction(ep: VAEntrypoint::Type) -> Option<HwCodecDirection> {
    match ep {
        VAEntrypoint::VAEntrypointVLD => Some(HwCodecDirection::Decode),
        VAEntrypoint::VAEntrypointEncSlice | VAEntrypoint::VAEntrypointEncSliceLP => {
            Some(HwCodecDirection::Encode)
        }
        _ => None,
    }
}

/// Find available DRM render nodes on the system.
fn find_render_nodes() -> Vec<String> {
    let mut nodes = Vec::new();
    for i in 128..136 {
        let path = format!("/dev/dri/renderD{i}");
        if Path::new(&path).exists() {
            nodes.push(path);
        }
    }
    nodes
}

/// Probe the system for VA-API hardware acceleration.
///
/// Tries each DRM render node and returns a report for the first one
/// that successfully initializes. Returns `None` if VA-API is not
/// available or no supported hardware is found.
///
/// Set `TARANG_VAAPI_DEVICE` to override the render node path.
pub fn probe_vaapi() -> Option<HwAccelReport> {
    let nodes = if let Ok(device) = std::env::var("TARANG_VAAPI_DEVICE") {
        vec![device]
    } else {
        find_render_nodes()
    };

    for node in &nodes {
        if let Some(report) = probe_node(node) {
            return Some(report);
        }
    }

    None
}

fn probe_node(render_node: &str) -> Option<HwAccelReport> {
    let path = Path::new(render_node);
    let display = Display::open_drm_display(path).ok()?;

    let vendor = display
        .query_vendor_string()
        .unwrap_or_else(|_| "unknown".to_string());
    let profiles = display.query_config_profiles().ok()?;

    let mut capabilities = Vec::new();

    for profile in profiles {
        let codec = match profile_to_codec(profile) {
            Some(c) => c,
            None => continue,
        };

        let entrypoints = match display.query_config_entrypoints(profile) {
            Ok(eps) => eps,
            Err(_) => continue,
        };

        for ep in entrypoints {
            if let Some(direction) = entrypoint_direction(ep) {
                capabilities.push(HwCodecCapability {
                    codec,
                    direction,
                    profile: profile_name(profile).to_string(),
                });
            }
        }
    }

    Some(HwAccelReport {
        driver_name: vendor,
        render_node: render_node.to_string(),
        capabilities,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_render_nodes_returns_existing() {
        let nodes = find_render_nodes();
        for node in &nodes {
            assert!(Path::new(node).exists());
        }
    }

    #[test]
    fn profile_mapping_h264() {
        assert_eq!(
            profile_to_codec(VAProfile::VAProfileH264Main),
            Some(VideoCodec::H264)
        );
        assert_eq!(
            profile_to_codec(VAProfile::VAProfileH264High),
            Some(VideoCodec::H264)
        );
        assert_eq!(
            profile_to_codec(VAProfile::VAProfileH264ConstrainedBaseline),
            Some(VideoCodec::H264)
        );
    }

    #[test]
    fn profile_mapping_hevc() {
        assert_eq!(
            profile_to_codec(VAProfile::VAProfileHEVCMain),
            Some(VideoCodec::H265)
        );
        assert_eq!(
            profile_to_codec(VAProfile::VAProfileHEVCMain10),
            Some(VideoCodec::H265)
        );
    }

    #[test]
    fn profile_mapping_vp9() {
        assert_eq!(
            profile_to_codec(VAProfile::VAProfileVP9Profile0),
            Some(VideoCodec::Vp9)
        );
        assert_eq!(
            profile_to_codec(VAProfile::VAProfileVP9Profile2),
            Some(VideoCodec::Vp9)
        );
    }

    #[test]
    fn profile_mapping_av1() {
        assert_eq!(
            profile_to_codec(VAProfile::VAProfileAV1Profile0),
            Some(VideoCodec::Av1)
        );
    }

    #[test]
    fn profile_mapping_vp8() {
        assert_eq!(
            profile_to_codec(VAProfile::VAProfileVP8Version0_3),
            Some(VideoCodec::Vp8)
        );
    }

    #[test]
    fn profile_mapping_unknown() {
        assert_eq!(profile_to_codec(VAProfile::VAProfileNone), None);
    }

    #[test]
    fn entrypoint_decode() {
        assert_eq!(
            entrypoint_direction(VAEntrypoint::VAEntrypointVLD),
            Some(HwCodecDirection::Decode)
        );
    }

    #[test]
    fn entrypoint_encode() {
        assert_eq!(
            entrypoint_direction(VAEntrypoint::VAEntrypointEncSlice),
            Some(HwCodecDirection::Encode)
        );
        assert_eq!(
            entrypoint_direction(VAEntrypoint::VAEntrypointEncSliceLP),
            Some(HwCodecDirection::Encode)
        );
    }

    #[test]
    fn entrypoint_other() {
        assert_eq!(
            entrypoint_direction(VAEntrypoint::VAEntrypointVideoProc),
            None
        );
    }

    #[test]
    fn hw_accel_report_queries() {
        let report = HwAccelReport {
            driver_name: "test".to_string(),
            render_node: "/dev/dri/renderD128".to_string(),
            capabilities: vec![
                HwCodecCapability {
                    codec: VideoCodec::H264,
                    direction: HwCodecDirection::Decode,
                    profile: "H264Main".to_string(),
                },
                HwCodecCapability {
                    codec: VideoCodec::H264,
                    direction: HwCodecDirection::Encode,
                    profile: "H264Main".to_string(),
                },
                HwCodecCapability {
                    codec: VideoCodec::Av1,
                    direction: HwCodecDirection::Decode,
                    profile: "AV1Profile0".to_string(),
                },
            ],
        };

        assert!(report.can_decode(VideoCodec::H264));
        assert!(report.can_encode(VideoCodec::H264));
        assert!(report.can_decode(VideoCodec::Av1));
        assert!(!report.can_encode(VideoCodec::Av1));
        assert!(!report.can_decode(VideoCodec::Vp8));
    }

    #[test]
    fn hw_accel_report_codec_lists() {
        let report = HwAccelReport {
            driver_name: "test".to_string(),
            render_node: "/dev/dri/renderD128".to_string(),
            capabilities: vec![
                HwCodecCapability {
                    codec: VideoCodec::H264,
                    direction: HwCodecDirection::Decode,
                    profile: "H264Main".to_string(),
                },
                HwCodecCapability {
                    codec: VideoCodec::Vp9,
                    direction: HwCodecDirection::Decode,
                    profile: "VP9Profile0".to_string(),
                },
                HwCodecCapability {
                    codec: VideoCodec::H264,
                    direction: HwCodecDirection::Encode,
                    profile: "H264Main".to_string(),
                },
            ],
        };

        let dec = report.decode_codecs();
        assert!(dec.contains(&VideoCodec::H264));
        assert!(dec.contains(&VideoCodec::Vp9));

        let enc = report.encode_codecs();
        assert!(enc.contains(&VideoCodec::H264));
        assert!(!enc.contains(&VideoCodec::Vp9));
    }

    #[test]
    fn hw_accel_report_empty() {
        let report = HwAccelReport {
            driver_name: "none".to_string(),
            render_node: "/dev/null".to_string(),
            capabilities: vec![],
        };

        assert!(!report.can_decode(VideoCodec::H264));
        assert!(!report.can_encode(VideoCodec::H264));
        assert!(report.decode_codecs().is_empty());
        assert!(report.encode_codecs().is_empty());
    }

    #[test]
    fn direction_display() {
        assert_eq!(HwCodecDirection::Decode.to_string(), "decode");
        assert_eq!(HwCodecDirection::Encode.to_string(), "encode");
    }

    #[test]
    fn profile_names() {
        assert_eq!(profile_name(VAProfile::VAProfileH264Main), "H264Main");
        assert_eq!(profile_name(VAProfile::VAProfileAV1Profile0), "AV1Profile0");
        assert_eq!(profile_name(VAProfile::VAProfileHEVCMain10), "HEVCMain10");
        assert_eq!(profile_name(VAProfile::VAProfileVP9Profile0), "VP9Profile0");
        assert_eq!(profile_name(999), "Unknown");
    }

    #[test]
    #[ignore] // Requires actual VA-API hardware
    fn probe_vaapi_on_real_hardware() {
        let report = probe_vaapi();
        if let Some(report) = report {
            assert!(!report.driver_name.is_empty());
            assert!(!report.render_node.is_empty());
            assert!(!report.capabilities.is_empty());
            println!("VA-API driver: {}", report.driver_name);
            println!("Render node: {}", report.render_node);
            for cap in &report.capabilities {
                println!("  {} {} ({})", cap.codec, cap.direction, cap.profile);
            }
        } else {
            println!("No VA-API hardware detected (expected on headless CI)");
        }
    }
}
