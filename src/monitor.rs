use std::time::{Duration, Instant};

use crate::config::{Config, ConfigError};
use crate::eval::Evaluator;
use crate::menu_model::{self, MenuModel};
use crate::metrics::Snapshot;
use crate::processes::{self, Culprit, ProcInfo};
use crate::status::{select_icon, IconKind, Status};

/// Result of one monitor tick: what the UI should display.
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    pub icon: IconKind,
    pub status: Status,
    pub menu: MenuModel,
}

/// Ties sampling, evaluation, culprit grouping and the menu model together.
pub struct Monitor {
    config: Config,
    config_error: Option<String>,
    evaluator: Evaluator,
    culprits: Vec<Culprit>,
    last_culprits_at: Option<Instant>,
}

impl Monitor {
    /// On `Err`, falls back to `Config::default()` and records the error.
    pub fn new(config: Result<Config, ConfigError>) -> Monitor {
        let (config, config_error) = match config {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(e.to_string())),
        };
        Monitor {
            config,
            config_error,
            evaluator: Evaluator::default(),
            culprits: Vec::new(),
            last_culprits_at: None,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// `Ok` replaces the config and clears the error; `Err` keeps the old
    /// config and records the error.
    pub fn set_config(&mut self, r: Result<Config, ConfigError>) {
        match r {
            Ok(c) => {
                self.config = c;
                self.config_error = None;
            }
            Err(e) => self.config_error = Some(e.to_string()),
        }
    }

    /// Evaluates `snap` and builds the menu. `procs` is called only when
    /// culprits have never been sampled or the refresh interval has elapsed.
    pub fn tick(
        &mut self,
        snap: &Snapshot,
        now: Instant,
        procs: impl FnOnce() -> Vec<ProcInfo>,
    ) -> Tick {
        let interval = Duration::from_secs(self.config.culprits_refresh_secs);
        let due = match self.last_culprits_at {
            None => true,
            Some(t) => now.saturating_duration_since(t) >= interval,
        };
        if due {
            self.culprits = processes::group(&procs(), self.config.culprits_count);
            self.last_culprits_at = Some(now);
        }
        let eval = self.evaluator.evaluate(snap, &self.config);
        let (icon, status) = select_icon(&eval.overall());
        let menu = menu_model::build(snap, &eval, &self.culprits, self.config_error.as_deref());
        Tick { icon, status, menu }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::PressureLevel;

    fn ok_snap() -> Snapshot {
        Snapshot {
            disk_free_bytes: 200_000_000_000,
            disk_total_bytes: 500_000_000_000,
            mem_total_bytes: 16 * 1_073_741_824,
            mem_available_bytes: 8 * 1_073_741_824,
            pressure: PressureLevel::Normal,
            swap_used_bytes: 0,
            ..Default::default()
        }
    }

    #[test]
    fn culprits_sampled_first_tick_then_throttled() {
        let mut m = Monitor::new(Ok(Config::default()));
        let t0 = Instant::now();
        let mut calls = 0;
        m.tick(&ok_snap(), t0, || { calls += 1; vec![] });
        m.tick(&ok_snap(), t0 + Duration::from_secs(5), || { calls += 1; vec![] });
        m.tick(&ok_snap(), t0 + Duration::from_secs(10), || { calls += 1; vec![] });
        assert_eq!(calls, 2);
    }

    #[test]
    fn ok_snapshot_gives_thumbs_up() {
        let t = Monitor::new(Ok(Config::default())).tick(&ok_snap(), Instant::now(), Vec::new);
        assert_eq!((t.icon, t.status), (IconKind::ThumbsUp, Status::Ok));
    }

    #[test]
    fn set_config_err_keeps_old_and_sets_error() {
        let c = Config { culprits_count: 3, ..Config::default() };
        let mut m = Monitor::new(Ok(c.clone()));
        m.set_config(Err(ConfigError::Parse("boom".into())));
        assert_eq!(m.config(), &c);
        assert_eq!(m.tick(&ok_snap(), Instant::now(), Vec::new).menu.config_error.as_deref(), Some("boom"));
        m.set_config(Ok(Config::default()));
        assert_eq!(m.tick(&ok_snap(), Instant::now(), Vec::new).menu.config_error, None);
    }

    #[test]
    fn new_with_err_uses_defaults() {
        assert_eq!(Monitor::new(Err(ConfigError::Io("x".into()))).config(), &Config::default());
    }
}
