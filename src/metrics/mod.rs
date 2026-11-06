//! System metrics: disk, memory, pressure and swap.

use crate::status::PressureLevel;

mod macos;
pub use macos::sample;

/// One point-in-time reading of the monitored resources.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    pub disk_free_bytes: u64,
    pub disk_total_bytes: u64,
    pub mem_total_bytes: u64,
    pub mem_available_bytes: u64,
    pub mem_compressed_bytes: u64,
    pub pressure: PressureLevel,
    pub swap_used_bytes: u64,
    pub swap_total_bytes: u64,
}

impl Snapshot {
    /// Available memory as a percentage of total; `100.0` when total is 0.
    pub fn available_pct(&self) -> f64 {
        if self.mem_total_bytes == 0 {
            return 100.0;
        }
        100.0 * self.mem_available_bytes as f64 / self.mem_total_bytes as f64
    }
}

/// Page counts from `vm_statistics64`, in pages.
pub struct VmPages {
    pub free: u64,
    pub inactive: u64,
    pub purgeable: u64,
    pub speculative: u64,
    pub compressor: u64,
}

/// Reclaimable memory: free + inactive + purgeable + speculative pages.
pub fn available_bytes(p: &VmPages, page_size: u64) -> u64 {
    (p.free + p.inactive + p.purgeable + p.speculative) * page_size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_bytes_sums_reclaimable() {
        let p = VmPages { free: 1, inactive: 2, purgeable: 3, speculative: 4, compressor: 100 };
        assert_eq!(available_bytes(&p, 16384), 10 * 16384);
    }

    #[test]
    fn available_pct_zero_total() {
        let s = Snapshot { mem_total_bytes: 0, mem_available_bytes: 0, ..Default::default() };
        assert_eq!(s.available_pct(), 100.0);
    }

    #[test]
    fn available_pct_normal() {
        let s = Snapshot { mem_total_bytes: 200, mem_available_bytes: 30, ..Default::default() };
        assert_eq!(s.available_pct(), 15.0);
    }

    #[test]
    fn sample_live_is_sane() {
        let s = sample().unwrap();
        assert!(s.mem_total_bytes > 0 && s.mem_available_bytes <= s.mem_total_bytes);
        assert!(s.disk_total_bytes > 0 && s.disk_free_bytes <= s.disk_total_bytes);
        assert!(s.swap_used_bytes <= s.swap_total_bytes);
    }
}
