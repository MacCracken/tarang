//! Hardware accelerator detection via ai-hwaccel
//!
//! Provides a unified view of available AI and media hardware accelerators
//! (GPU, NPU, TPU) by wrapping the `ai-hwaccel` crate's
//! [`AcceleratorRegistry`]. This module is behind the `hwaccel` feature flag.
//!
//! Use [`probe_hardware`] for a quick summary or [`accelerator_registry`]
//! for full access to the underlying registry.

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
        .map(|p| profile_to_info(p))
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
        // Should always have system memory > 0
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
            memory_bytes: 8 * 1024 * 1024 * 1024, // 8GB
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
}
