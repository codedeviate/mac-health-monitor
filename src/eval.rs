use crate::config::Config;
use crate::metrics::Snapshot;
use crate::status::{self, Status};
use crate::units::{GB, GIB};

/// Which way a value gets worse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Direction {
    HigherIsWorse,
    LowerIsWorse,
}

/// Warning and critical thresholds for one parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    pub warn: f64,
    pub crit: f64,
    pub direction: Direction,
}

/// Classifies a value. Comparisons are strict: a value exactly at a threshold is not worse.
pub fn classify(value: f64, t: &Thresholds) -> Status {
    let (past_crit, past_warn) = match t.direction {
        Direction::HigherIsWorse => (value > t.crit, value > t.warn),
        Direction::LowerIsWorse => (value < t.crit, value < t.warn),
    };
    if past_crit {
        Status::Critical
    } else if past_warn {
        Status::Warning
    } else {
        Status::Ok
    }
}

/// Moves both thresholds toward the better side by `margin_pct` percent of their own value.
pub fn relaxed(t: &Thresholds, margin_pct: f64) -> Thresholds {
    let m = margin_pct / 100.0;
    let k = match t.direction {
        Direction::HigherIsWorse => 1.0 - m,
        Direction::LowerIsWorse => 1.0 + m,
    };
    Thresholds { warn: t.warn * k, crit: t.crit * k, direction: t.direction }
}

/// Debounces one parameter: worsening is immediate, improving needs two
/// consecutive samples that are better even under the relaxed thresholds.
#[derive(Debug, Clone, Default)]
pub struct Hysteresis {
    current: Status,
    pending: u8,
}

impl Hysteresis {
    pub fn update(&mut self, raw: Status, relaxed: Status) -> Status {
        if raw > self.current {
            self.current = raw;
            self.pending = 0;
        } else if relaxed < self.current {
            self.pending += 1;
            if self.pending >= 2 {
                self.current = relaxed;
                self.pending = 0;
            }
        } else {
            self.pending = 0;
        }
        self.current
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    pub disk: Status,
    pub memory: Status,
    pub swap: Status,
    /// Kernel pressure level, informational only (no hysteresis).
    pub pressure: Status,
}

impl Evaluation {
    pub fn overall(&self) -> status::Overall {
        status::Overall { disk: self.disk, memory: self.memory, swap: self.swap }
    }
}

#[derive(Debug, Default)]
pub struct Evaluator {
    disk: Hysteresis,
    memory: Hysteresis,
    swap: Hysteresis,
}

impl Evaluator {
    pub fn evaluate(&mut self, s: &Snapshot, c: &Config) -> Evaluation {
        let m = c.hysteresis_margin_pct;
        let pressure = s.pressure.status();

        let disk_t = Thresholds {
            warn: c.disk.warn_free_gb,
            crit: c.disk.crit_free_gb,
            direction: Direction::LowerIsWorse,
        };
        let disk_v = s.disk_free_bytes as f64 / GB;
        let disk = self
            .disk
            .update(classify(disk_v, &disk_t), classify(disk_v, &relaxed(&disk_t, m)));

        let swap_t = Thresholds {
            warn: c.swap.warn_used_gb,
            crit: c.swap.crit_used_gb,
            direction: Direction::HigherIsWorse,
        };
        let swap_v = s.swap_used_bytes as f64 / GIB;
        let swap = self
            .swap
            .update(classify(swap_v, &swap_t), classify(swap_v, &relaxed(&swap_t, m)));

        let mem_t = Thresholds {
            warn: c.memory.warn_available_pct,
            crit: c.memory.crit_available_pct,
            direction: Direction::LowerIsWorse,
        };
        let p = if c.memory.use_pressure_level { pressure } else { Status::Ok };
        let mem_v = s.available_pct();
        let memory = self.memory.update(
            p.max(classify(mem_v, &mem_t)),
            p.max(classify(mem_v, &relaxed(&mem_t, m))),
        );

        Evaluation { disk, memory, swap, pressure }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::{IconKind, PressureLevel};

    const SWAP: Thresholds = Thresholds { warn: 4.0, crit: 10.0, direction: Direction::HigherIsWorse };
    const DISK: Thresholds = Thresholds { warn: 25.0, crit: 10.0, direction: Direction::LowerIsWorse };

    #[test]
    fn classify_basic() {
        assert_eq!(classify(1.0, &SWAP), Status::Ok);
        assert_eq!(classify(5.0, &SWAP), Status::Warning);
        assert_eq!(classify(20.2, &SWAP), Status::Critical);
        assert_eq!(classify(100.0, &DISK), Status::Ok);
        assert_eq!(classify(22.0, &DISK), Status::Warning);
        assert_eq!(classify(5.0, &DISK), Status::Critical);
    }

    #[test]
    fn classify_boundary_is_not_worse() {
        assert_eq!(classify(4.0, &SWAP), Status::Ok);
        assert_eq!(classify(25.0, &DISK), Status::Ok);
    }

    #[test]
    fn relaxed_moves_toward_better() {
        let r = relaxed(&SWAP, 5.0);
        assert!((r.warn - 3.8).abs() < 1e-9 && (r.crit - 9.5).abs() < 1e-9);
        let r = relaxed(&DISK, 5.0);
        assert!((r.warn - 26.25).abs() < 1e-9 && (r.crit - 10.5).abs() < 1e-9);
    }

    #[test]
    fn hysteresis_worsens_immediately() {
        let mut h = Hysteresis::default();
        assert_eq!(h.update(Status::Critical, Status::Critical), Status::Critical);
    }

    #[test]
    fn hysteresis_needs_two_samples_past_margin() {
        let mut h = Hysteresis::default();
        h.update(Status::Warning, Status::Warning);
        assert_eq!(h.update(Status::Ok, Status::Ok), Status::Warning);
        assert_eq!(h.update(Status::Ok, Status::Ok), Status::Ok);
    }

    #[test]
    fn hysteresis_resets_inside_margin_band() {
        let mut h = Hysteresis::default();
        h.update(Status::Warning, Status::Warning);
        h.update(Status::Ok, Status::Ok); // pending 1
        h.update(Status::Ok, Status::Warning); // inside band: reset
        assert_eq!(h.update(Status::Ok, Status::Ok), Status::Warning);
    }

    #[test]
    fn evaluate_calibration_snapshot_is_red_triangle() {
        // the user's machine on 2026-11-06: swap 20.2 GiB, pressure warn, disk 22 GB, 35% available
        let s = Snapshot {
            disk_free_bytes: 22_000_000_000,
            disk_total_bytes: 494_000_000_000,
            mem_total_bytes: 18 * GIB as u64,
            mem_available_bytes: (18.0 * GIB * 0.35) as u64,
            mem_compressed_bytes: 0,
            pressure: PressureLevel::Warning,
            swap_used_bytes: (20.2 * GIB) as u64,
            swap_total_bytes: (21.0 * GIB) as u64,
        };
        let e = Evaluator::default().evaluate(&s, &Config::default());
        assert_eq!((e.disk, e.memory, e.swap), (Status::Warning, Status::Warning, Status::Critical));
        assert_eq!(status::select_icon(&e.overall()), (IconKind::Triangle, Status::Critical));
    }

    #[test]
    fn pressure_ignored_when_disabled() {
        let mut c = Config::default();
        c.memory.use_pressure_level = false;
        let s = Snapshot {
            pressure: PressureLevel::Critical,
            mem_total_bytes: 100,
            mem_available_bytes: 50,
            disk_free_bytes: 1_000_000_000_000,
            disk_total_bytes: 1_000_000_000_000,
            ..Default::default()
        };
        assert_eq!(Evaluator::default().evaluate(&s, &c).memory, Status::Ok);
    }
}
