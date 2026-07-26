//! Rough RAM/VRAM fit estimation for a model's on-disk size, feeding M3's
//! "hardware-honesty labels" (Fits / May be slow / Won't fit) shown next to
//! catalog entries in the models panel.
//!
//! This is deliberately a heuristic: quantization, context size, and KV
//! cache all shift real memory needs beyond a model's file size. Always
//! present the label as an estimate; never block an action on it.

use sysinfo::System;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitLabel {
    Fits,
    MaySlow,
    WontFit,
}

impl FitLabel {
    pub fn label(self) -> &'static str {
        match self {
            FitLabel::Fits => "Fits",
            FitLabel::MaySlow => "May be slow",
            FitLabel::WontFit => "Won't fit",
        }
    }
}

/// `size_bytes <= 0.7x` available memory: Fits. `<= 1.0x`: May be slow.
/// Above that (or no memory info available): Won't fit.
pub fn fit_label(size_bytes: u64, available_bytes: u64) -> FitLabel {
    if available_bytes == 0 {
        return FitLabel::WontFit;
    }
    let ratio = size_bytes as f64 / available_bytes as f64;
    if ratio <= 0.7 {
        FitLabel::Fits
    } else if ratio <= 1.0 {
        FitLabel::MaySlow
    } else {
        FitLabel::WontFit
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SystemMemory {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

impl SystemMemory {
    /// Reads current system memory. On Apple Silicon, RAM is unified with
    /// VRAM, so `available_bytes` is the budget for both.
    pub fn probe() -> Self {
        let mut sys = System::new();
        sys.refresh_memory();
        Self {
            total_bytes: sys.total_memory(),
            available_bytes: sys.available_memory(),
        }
    }

    pub fn fit_label_for(&self, size_bytes: u64) -> FitLabel {
        fit_label(size_bytes, self.available_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn small_model_fits() {
        assert_eq!(fit_label(4 * GIB, 16 * GIB), FitLabel::Fits);
    }

    #[test]
    fn borderline_model_may_be_slow() {
        assert_eq!(fit_label(12 * GIB, 16 * GIB), FitLabel::MaySlow);
    }

    #[test]
    fn oversized_model_wont_fit() {
        assert_eq!(fit_label(32 * GIB, 16 * GIB), FitLabel::WontFit);
    }

    #[test]
    fn thresholds_are_inclusive() {
        assert_eq!(fit_label(7 * GIB, 10 * GIB), FitLabel::Fits); // exactly 0.7x
        assert_eq!(fit_label(10 * GIB, 10 * GIB), FitLabel::MaySlow); // exactly 1.0x
    }

    #[test]
    fn zero_available_memory_never_fits() {
        assert_eq!(fit_label(1, 0), FitLabel::WontFit);
    }

    #[test]
    fn probe_returns_nonzero_memory_on_this_machine() {
        let mem = SystemMemory::probe();
        assert!(mem.total_bytes > 0);
    }
}
