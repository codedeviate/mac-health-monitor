use crate::eval::Evaluation;
use crate::metrics::Snapshot;
use crate::processes::Culprit;
use crate::status::Status;
use crate::units::{GB, GIB};

/// Marker shown at the start of a menu row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Indicator {
    Status(Status),
    Info,
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub indicator: Indicator,
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MenuModel {
    pub params: Vec<Row>,
    pub culprits: Vec<Row>,
    pub config_error: Option<String>,
}

/// Decimal gigabytes with one decimal place, e.g. `142.3 GB`.
pub fn fmt_gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / GB)
}

/// Binary gibibytes with one decimal place, labelled "GB", e.g. `2.1 GB`.
pub fn fmt_gib(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / GIB)
}

fn row(indicator: Indicator, label: &str, value: String) -> Row {
    Row { indicator, label: label.to_string(), value }
}

pub fn build(s: &Snapshot, e: &Evaluation, culprits: &[Culprit], config_error: Option<&str>) -> MenuModel {
    let params = vec![
        row(
            Indicator::Status(e.disk),
            "Disk free",
            format!("{} of {}", fmt_gb(s.disk_free_bytes), fmt_gb(s.disk_total_bytes)),
        ),
        row(
            Indicator::Status(e.memory),
            "Memory available",
            format!(
                "{} of {} ({:.0}%)",
                fmt_gib(s.mem_available_bytes),
                fmt_gib(s.mem_total_bytes),
                s.available_pct()
            ),
        ),
        row(Indicator::Status(e.pressure), "Memory pressure", s.pressure.label().to_string()),
        row(
            Indicator::Status(e.swap),
            "Swap used",
            format!("{} of {}", fmt_gib(s.swap_used_bytes), fmt_gib(s.swap_total_bytes)),
        ),
        row(Indicator::Info, "Compressed", fmt_gib(s.mem_compressed_bytes)),
    ];
    let culprits = culprits
        .iter()
        .map(|c| {
            let label = if c.count > 1 { format!("{} (×{})", c.label, c.count) } else { c.label.clone() };
            Row { indicator: Indicator::None, label, value: fmt_gib(c.bytes) }
        })
        .collect();
    MenuModel { params, culprits, config_error: config_error.map(str::to_string) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::PressureLevel;

    fn fixture_snapshot() -> Snapshot {
        Snapshot {
            disk_free_bytes: (142.3 * GB) as u64,
            disk_total_bytes: (494.4 * GB) as u64,
            mem_total_bytes: (18.0 * GIB) as u64,
            mem_available_bytes: (2.1 * GIB) as u64,
            mem_compressed_bytes: (3.4 * GIB) as u64,
            pressure: PressureLevel::Normal,
            swap_used_bytes: (1.2 * GIB) as u64,
            swap_total_bytes: (2.0 * GIB) as u64,
        }
    }

    fn fixture_eval() -> Evaluation {
        Evaluation { disk: Status::Ok, memory: Status::Ok, swap: Status::Ok, pressure: Status::Ok }
    }

    #[test] fn fmt_units() { assert_eq!(fmt_gb(142_300_000_000), "142.3 GB"); assert_eq!(fmt_gib(2_254_857_830), "2.1 GB"); assert_eq!(fmt_gib(0), "0.0 GB"); }
    #[test] fn params_rows_in_order_with_values() {
        let m = build(&fixture_snapshot(), &fixture_eval(), &[], None);
        let labels: Vec<_> = m.params.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Disk free", "Memory available", "Memory pressure", "Swap used", "Compressed"]);
        assert_eq!(m.params[0].value, "142.3 GB of 494.4 GB");
        assert_eq!(m.params[1].value, "2.1 GB of 18.0 GB (12%)");
        assert_eq!(m.params[4].indicator, Indicator::Info);
    }
    #[test] fn swap_zero_total_formats() {
        let s = Snapshot { swap_used_bytes: 0, swap_total_bytes: 0, ..fixture_snapshot() };
        assert_eq!(build(&s, &fixture_eval(), &[], None).params[3].value, "0.0 GB of 0.0 GB");
    }
    #[test] fn culprit_count_suffix() {
        let c = [Culprit { label: "claude".into(), bytes: 0, count: 3 }, Culprit { label: "Arc".into(), bytes: 0, count: 1 }];
        let m = build(&fixture_snapshot(), &fixture_eval(), &c, Some("bad"));
        assert_eq!(m.culprits[0].label, "claude (×3)"); assert_eq!(m.culprits[1].label, "Arc");
        assert_eq!(m.config_error.as_deref(), Some("bad"));
    }
}
