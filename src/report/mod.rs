//! Health report: everything gathered for one report, and the pure logic built on it.

pub mod findings;

use crate::config::Config;
use crate::diag::Incident;
use crate::eval::Evaluation;
use crate::history::HistorySample;
use crate::metrics::Snapshot;
use crate::processes::{Culprit, ProcInfo};

/// All inputs to one health report. Built by the I/O layer; read by pure functions.
#[derive(Debug, Clone)]
pub struct ReportInput {
    /// Unix seconds at which the report is generated.
    pub now: i64,
    pub boot_time: Option<i64>,
    /// e.g. `"Mac15,7 · macOS 27.2.1 · 18.0 GB RAM"`.
    pub machine: String,
    pub config: Config,
    pub snapshot: Snapshot,
    pub eval: Evaluation,
    /// Top 10 apps, grouped from a fresh process walk.
    pub apps: Vec<Culprit>,
    /// The full current process sample.
    pub procs: Vec<ProcInfo>,
    /// Incidents in the report window, oldest first.
    pub incidents: Vec<Incident>,
    /// History samples, oldest first.
    pub history: Vec<HistorySample>,
    /// Files read.
    pub sources: Vec<String>,
    /// Problems met while gathering (unreadable files and the like).
    pub notes: Vec<String>,
}
