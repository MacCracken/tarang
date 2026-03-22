//! Hardware accelerator detection via ai-hwaccel
//!
//! Provides a unified view of available AI and media hardware accelerators
//! (GPU, NPU, TPU) by wrapping the `ai-hwaccel` crate's
//! [`AcceleratorRegistry`]. This module is behind the `hwaccel` feature flag.
//!
//! Use [`probe_hardware`] for a quick summary or [`accelerator_registry`]
//! for full access to the underlying registry.
//!
//! ## Capability matching
//!
//! [`CodecCapabilities`] maps detected hardware to tarang codec backends,
//! answering "which codecs can this hardware decode/encode?":
//!
//! ```rust,no_run
//! # #[cfg(feature = "hwaccel")]
//! # {
//! let caps = tarang::hwaccel::probe_codec_capabilities();
//! for entry in &caps.decode {
//!     println!("{} via {}", entry.codec, entry.backend);
//! }
//! # }
//! ```

use crate::core::VideoCodec;
use ai_hwaccel::{AcceleratorFamily, AcceleratorProfile, AcceleratorRegistry, AcceleratorType};
use std::fmt;

/// Summary of a single detected accelerator, tailored for tarang's needs.
#[derive(Debug, Clone)]
pub struct AcceleratorInfo {
    /// Human-readable name (e.g. "CUDA GPU #0", "Vulkan: AMD Radeon")
    pub name: String,
    /// Hardware family
    pub family: AcceleratorFamily,
    /// Total device memory in bytes (0 for CPU-only)
    pub memory_bytes: u64,
    /// Whether this is a Vulkan compute device
    pub vulkan_compute: bool,
}

impl fmt::Display for AcceleratorInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mem_mb = self.memory_bytes / (1024 * 1024);
        write!(f, "{} ({}, {}MB", self.name, self.family, mem_mb)?;
        if self.vulkan_compute {
            write!(f, ", vulkan")?;
        }
        write!(f, ")")
    }
}

/// Full hardware report from ai-hwaccel, with tarang-specific helpers.
#[derive(Debug)]
pub struct HardwareReport {
    /// All detected accelerators
    pub accelerators: Vec<AcceleratorInfo>,
    /// Total accelerator memory across all devices (bytes)
    pub total_accel_memory: u64,
    /// Total system memory (bytes)
    pub total_system_memory: u64,
}

impl HardwareReport {
    /// Whether any GPU with Vulkan compute support was found.
    pub fn has_vulkan_compute(&self) -> bool {
        self.accelerators.iter().any(|a| a.vulkan_compute)
    }

    /// Whether any dedicated GPU is available.
    pub fn has_gpu(&self) -> bool {
        self.accelerators
            .iter()
            .any(|a| a.family == AcceleratorFamily::Gpu)
    }

    /// Whether any NPU/TPU/ASIC accelerator is available.
    pub fn has_npu(&self) -> bool {
        self.accelerators.iter().any(|a| {
            matches!(
                a.family,
                AcceleratorFamily::Npu | AcceleratorFamily::Tpu | AcceleratorFamily::AiAsic
            )
        })
    }

    /// Get the best accelerator (most memory) or None if only CPU.
    pub fn best_accelerator(&self) -> Option<&AcceleratorInfo> {
        self.accelerators
            .iter()
            .filter(|a| a.family != AcceleratorFamily::Cpu)
            .max_by_key(|a| a.memory_bytes)
    }
}

impl fmt::Display for HardwareReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Hardware accelerators:")?;
        if self.accelerators.is_empty() {
            writeln!(f, "  (none detected)")?;
        } else {
            for accel in &self.accelerators {
                writeln!(f, "  {accel}")?;
            }
        }
        let sys_gb = self.total_system_memory as f64 / (1024.0 * 1024.0 * 1024.0);
        let accel_gb = self.total_accel_memory as f64 / (1024.0 * 1024.0 * 1024.0);
        writeln!(f, "System memory:      {sys_gb:.1} GB")?;
        writeln!(f, "Accelerator memory: {accel_gb:.1} GB")?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Capability matching: map accelerator profiles → tarang codec features
// ---------------------------------------------------------------------------

/// How a codec backend is accelerated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecBackendKind {
    /// Software decoder/encoder compiled via feature flag.
    Software,
    /// VA-API hardware path (Linux DRM render node).
    Vaapi,
}

impl fmt::Display for CodecBackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Software => write!(f, "software"),
            Self::Vaapi => write!(f, "vaapi"),
        }
    }
}

/// A single codec capability entry — one codec in one direction via one backend.
#[derive(Debug, Clone)]
pub struct CodecEntry {
    pub codec: VideoCodec,
    pub backend: CodecBackendKind,
    /// Name of the software library or hardware driver.
    pub driver: String,
}

impl fmt::Display for CodecEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} via {} ({})", self.codec, self.backend, self.driver)
    }
}

/// Unified view of all available codec decode/encode paths, combining
/// compile-time feature flags, VA-API probing, and ai-hwaccel detection.
#[derive(Debug, Clone)]
pub struct CodecCapabilities {
    /// Available decode paths (sorted: hardware first, then software).
    pub decode: Vec<CodecEntry>,
    /// Available encode paths (sorted: hardware first, then software).
    pub encode: Vec<CodecEntry>,
}

impl CodecCapabilities {
    /// Check if any decode path exists for the given codec.
    pub fn can_decode(&self, codec: VideoCodec) -> bool {
        self.decode.iter().any(|e| e.codec == codec)
    }

    /// Check if any encode path exists for the given codec.
    pub fn can_encode(&self, codec: VideoCodec) -> bool {
        self.encode.iter().any(|e| e.codec == codec)
    }

    /// Best decode entry for a codec (hardware preferred over software).
    pub fn best_decode(&self, codec: VideoCodec) -> Option<&CodecEntry> {
        // List is sorted hw-first, so first match is best.
        self.decode.iter().find(|e| e.codec == codec)
    }

    /// Best encode entry for a codec (hardware preferred over software).
    pub fn best_encode(&self, codec: VideoCodec) -> Option<&CodecEntry> {
        self.encode.iter().find(|e| e.codec == codec)
    }

    /// All decode entries for a specific codec.
    pub fn decode_for(&self, codec: VideoCodec) -> Vec<&CodecEntry> {
        self.decode.iter().filter(|e| e.codec == codec).collect()
    }

    /// All encode entries for a specific codec.
    pub fn encode_for(&self, codec: VideoCodec) -> Vec<&CodecEntry> {
        self.encode.iter().filter(|e| e.codec == codec).collect()
    }
}

impl fmt::Display for CodecCapabilities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Decode:")?;
        if self.decode.is_empty() {
            writeln!(f, "  (none)")?;
        } else {
            for entry in &self.decode {
                writeln!(f, "  {entry}")?;
            }
        }
        writeln!(f, "Encode:")?;
        if self.encode.is_empty() {
            writeln!(f, "  (none)")?;
        } else {
            for entry in &self.encode {
                writeln!(f, "  {entry}")?;
            }
        }
        Ok(())
    }
}

/// Probe the system and build a unified [`CodecCapabilities`] report.
///
/// Combines three sources:
/// 1. **Compile-time feature flags** — which software backends are linked.
/// 2. **VA-API probing** (if `vaapi` feature enabled) — hardware codec support.
/// 3. **ai-hwaccel detection** — confirms GPU/NPU presence for future Vulkan
///    compute paths.
///
/// The returned lists are sorted with hardware entries before software entries.
pub fn probe_codec_capabilities() -> CodecCapabilities {
    let mut decode = Vec::new();
    let mut encode = Vec::new();

    // --- Hardware: VA-API ---
    #[cfg(all(target_os = "linux", feature = "vaapi"))]
    if let Some(vaapi) = crate::video::probe_vaapi() {
        use crate::video::HwCodecDirection;
        for cap in &vaapi.capabilities {
            let entry = CodecEntry {
                codec: cap.codec,
                backend: CodecBackendKind::Vaapi,
                driver: vaapi.driver_name.clone(),
            };
            match cap.direction {
                HwCodecDirection::Decode => decode.push(entry),
                HwCodecDirection::Encode => encode.push(entry),
            }
        }
        // Deduplicate VA-API entries (multiple profiles for same codec).
        decode.dedup_by(|a, b| a.codec == b.codec && a.backend == b.backend);
        encode.dedup_by(|a, b| a.codec == b.codec && a.backend == b.backend);
    }

    // --- Software decoders (feature-gated) ---
    if cfg!(feature = "dav1d") {
        decode.push(CodecEntry {
            codec: VideoCodec::Av1,
            backend: CodecBackendKind::Software,
            driver: "dav1d".into(),
        });
    }
    if cfg!(feature = "openh264") {
        decode.push(CodecEntry {
            codec: VideoCodec::H264,
            backend: CodecBackendKind::Software,
            driver: "openh264".into(),
        });
    }
    if cfg!(feature = "vpx") {
        decode.push(CodecEntry {
            codec: VideoCodec::Vp8,
            backend: CodecBackendKind::Software,
            driver: "libvpx".into(),
        });
        decode.push(CodecEntry {
            codec: VideoCodec::Vp9,
            backend: CodecBackendKind::Software,
            driver: "libvpx".into(),
        });
    }

    // --- Software encoders (feature-gated) ---
    if cfg!(feature = "rav1e") {
        encode.push(CodecEntry {
            codec: VideoCodec::Av1,
            backend: CodecBackendKind::Software,
            driver: "rav1e".into(),
        });
    }
    if cfg!(feature = "openh264-enc") {
        encode.push(CodecEntry {
            codec: VideoCodec::H264,
            backend: CodecBackendKind::Software,
            driver: "openh264".into(),
        });
    }
    if cfg!(feature = "vpx-enc") {
        encode.push(CodecEntry {
            codec: VideoCodec::Vp8,
            backend: CodecBackendKind::Software,
            driver: "libvpx".into(),
        });
        encode.push(CodecEntry {
            codec: VideoCodec::Vp9,
            backend: CodecBackendKind::Software,
            driver: "libvpx".into(),
        });
    }

    CodecCapabilities { decode, encode }
}

/// Recommend the best decode backend for a given codec based on hardware.
///
/// Returns `Some((backend_kind, driver_name))` or `None` if the codec is
/// unsupported with the current feature set and hardware.
pub fn recommend_decode_backend(
    codec: VideoCodec,
    caps: &CodecCapabilities,
) -> Option<(CodecBackendKind, String)> {
    caps.best_decode(codec)
        .map(|e| (e.backend, e.driver.clone()))
}

/// Recommend the best encode backend for a given codec based on hardware.
///
/// Prefers hardware encoding (VA-API) over software when available.
pub fn recommend_encode_backend(
    codec: VideoCodec,
    caps: &CodecCapabilities,
) -> Option<(CodecBackendKind, String)> {
    caps.best_encode(codec)
        .map(|e| (e.backend, e.driver.clone()))
}

fn is_vulkan(accel: &AcceleratorType) -> bool {
    matches!(accel, AcceleratorType::VulkanGpu { .. })
}

fn profile_to_info(profile: &AcceleratorProfile) -> AcceleratorInfo {
    AcceleratorInfo {
        name: profile.accelerator.to_string(),
        family: profile.accelerator.family(),
        memory_bytes: profile.memory_bytes,
        vulkan_compute: is_vulkan(&profile.accelerator),
    }
}

/// Detect all hardware accelerators on the system.
///
/// Returns a [`HardwareReport`] summarizing available GPUs, NPUs, and system
/// memory. This is a relatively expensive operation (probes sysfs, device
/// nodes, etc.) so cache the result if calling repeatedly.
pub fn probe_hardware() -> HardwareReport {
    let registry = AcceleratorRegistry::detect();
    let accelerators: Vec<AcceleratorInfo> = registry
        .all_profiles()
        .iter()
        .filter(|p| p.available)
        .map(profile_to_info)
        .collect();
    let total_accel_memory = registry.total_accelerator_memory();
    let total_system_memory = registry.total_memory();

    HardwareReport {
        accelerators,
        total_accel_memory,
        total_system_memory,
    }
}

/// Get the raw ai-hwaccel registry for advanced queries.
///
/// Use this when you need access to quantization suggestions,
/// sharding plans, or filtered queries not exposed by [`HardwareReport`].
pub fn accelerator_registry() -> AcceleratorRegistry {
    AcceleratorRegistry::detect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_returns_report() {
        let report = probe_hardware();
        assert!(report.total_system_memory > 0);
    }

    #[test]
    fn display_report() {
        let report = probe_hardware();
        let text = format!("{report}");
        assert!(text.contains("Hardware accelerators:"));
        assert!(text.contains("System memory:"));
    }

    #[test]
    fn accelerator_info_display() {
        let info = AcceleratorInfo {
            name: "Test GPU".to_string(),
            family: AcceleratorFamily::Gpu,
            memory_bytes: 8 * 1024 * 1024 * 1024,
            vulkan_compute: true,
        };
        let text = format!("{info}");
        assert!(text.contains("Test GPU"));
        assert!(text.contains("vulkan"));
        assert!(text.contains("8192MB"));
    }

    #[test]
    fn empty_report_helpers() {
        let report = HardwareReport {
            accelerators: vec![],
            total_accel_memory: 0,
            total_system_memory: 16 * 1024 * 1024 * 1024,
        };
        assert!(!report.has_gpu());
        assert!(!report.has_npu());
        assert!(!report.has_vulkan_compute());
        assert!(report.best_accelerator().is_none());
    }

    #[test]
    fn best_accelerator_picks_most_memory() {
        let report = HardwareReport {
            accelerators: vec![
                AcceleratorInfo {
                    name: "Small GPU".into(),
                    family: AcceleratorFamily::Gpu,
                    memory_bytes: 4 * 1024 * 1024 * 1024,
                    vulkan_compute: true,
                },
                AcceleratorInfo {
                    name: "Big GPU".into(),
                    family: AcceleratorFamily::Gpu,
                    memory_bytes: 24 * 1024 * 1024 * 1024,
                    vulkan_compute: true,
                },
                AcceleratorInfo {
                    name: "CPU".into(),
                    family: AcceleratorFamily::Cpu,
                    memory_bytes: 64 * 1024 * 1024 * 1024,
                    vulkan_compute: false,
                },
            ],
            total_accel_memory: 28 * 1024 * 1024 * 1024,
            total_system_memory: 64 * 1024 * 1024 * 1024,
        };
        let best = report.best_accelerator().unwrap();
        assert_eq!(best.name, "Big GPU");
        assert!(report.has_gpu());
    }

    // --- CodecCapabilities tests ---

    #[test]
    fn probe_codec_capabilities_returns_entries() {
        let caps = probe_codec_capabilities();
        // Software decoders depend on feature flags, but the function shouldn't panic.
        let _ = format!("{caps}");
    }

    #[test]
    fn codec_capabilities_queries() {
        let caps = CodecCapabilities {
            decode: vec![
                CodecEntry {
                    codec: VideoCodec::H264,
                    backend: CodecBackendKind::Vaapi,
                    driver: "i965".into(),
                },
                CodecEntry {
                    codec: VideoCodec::H264,
                    backend: CodecBackendKind::Software,
                    driver: "openh264".into(),
                },
                CodecEntry {
                    codec: VideoCodec::Av1,
                    backend: CodecBackendKind::Software,
                    driver: "dav1d".into(),
                },
            ],
            encode: vec![CodecEntry {
                codec: VideoCodec::H264,
                backend: CodecBackendKind::Vaapi,
                driver: "i965".into(),
            }],
        };

        assert!(caps.can_decode(VideoCodec::H264));
        assert!(caps.can_decode(VideoCodec::Av1));
        assert!(!caps.can_decode(VideoCodec::Vp9));
        assert!(caps.can_encode(VideoCodec::H264));
        assert!(!caps.can_encode(VideoCodec::Av1));

        // best_decode prefers hardware (first in list)
        let best = caps.best_decode(VideoCodec::H264).unwrap();
        assert_eq!(best.backend, CodecBackendKind::Vaapi);

        // decode_for returns all entries
        assert_eq!(caps.decode_for(VideoCodec::H264).len(), 2);
        assert_eq!(caps.decode_for(VideoCodec::Av1).len(), 1);
        assert_eq!(caps.decode_for(VideoCodec::Vp9).len(), 0);
    }

    #[test]
    fn recommend_decode_prefers_vaapi() {
        let caps = CodecCapabilities {
            decode: vec![
                CodecEntry {
                    codec: VideoCodec::H264,
                    backend: CodecBackendKind::Vaapi,
                    driver: "i965".into(),
                },
                CodecEntry {
                    codec: VideoCodec::H264,
                    backend: CodecBackendKind::Software,
                    driver: "openh264".into(),
                },
            ],
            encode: vec![],
        };
        let (kind, driver) = recommend_decode_backend(VideoCodec::H264, &caps).unwrap();
        assert_eq!(kind, CodecBackendKind::Vaapi);
        assert_eq!(driver, "i965");
    }

    #[test]
    fn recommend_decode_falls_back_to_software() {
        let caps = CodecCapabilities {
            decode: vec![CodecEntry {
                codec: VideoCodec::Av1,
                backend: CodecBackendKind::Software,
                driver: "dav1d".into(),
            }],
            encode: vec![],
        };
        let (kind, _) = recommend_decode_backend(VideoCodec::Av1, &caps).unwrap();
        assert_eq!(kind, CodecBackendKind::Software);
    }

    #[test]
    fn recommend_encode_prefers_vaapi() {
        let caps = CodecCapabilities {
            decode: vec![],
            encode: vec![
                CodecEntry {
                    codec: VideoCodec::H264,
                    backend: CodecBackendKind::Vaapi,
                    driver: "i965".into(),
                },
                CodecEntry {
                    codec: VideoCodec::H264,
                    backend: CodecBackendKind::Software,
                    driver: "openh264".into(),
                },
            ],
        };
        let (kind, _) = recommend_encode_backend(VideoCodec::H264, &caps).unwrap();
        assert_eq!(kind, CodecBackendKind::Vaapi);
    }

    #[test]
    fn recommend_returns_none_for_unsupported() {
        let caps = CodecCapabilities {
            decode: vec![],
            encode: vec![],
        };
        assert!(recommend_decode_backend(VideoCodec::H265, &caps).is_none());
        assert!(recommend_encode_backend(VideoCodec::H265, &caps).is_none());
    }

    #[test]
    fn codec_capabilities_display() {
        let caps = CodecCapabilities {
            decode: vec![CodecEntry {
                codec: VideoCodec::H264,
                backend: CodecBackendKind::Vaapi,
                driver: "i965".into(),
            }],
            encode: vec![],
        };
        let text = format!("{caps}");
        assert!(text.contains("Decode:"));
        assert!(text.contains("H.264 via vaapi (i965)"));
        assert!(text.contains("Encode:"));
    }

    #[test]
    fn codec_backend_kind_display() {
        assert_eq!(CodecBackendKind::Software.to_string(), "software");
        assert_eq!(CodecBackendKind::Vaapi.to_string(), "vaapi");
    }
}
