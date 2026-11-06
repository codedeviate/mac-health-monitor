//! TOML configuration: defaults, validation and load-or-create.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub refresh_secs: u64,
    pub culprits_refresh_secs: u64,
    pub culprits_count: usize,
    pub hysteresis_margin_pct: f64,
    pub disk: DiskConfig,
    pub memory: MemoryConfig,
    pub swap: SwapConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiskConfig {
    pub warn_free_gb: f64,
    pub crit_free_gb: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryConfig {
    pub warn_available_pct: f64,
    pub crit_available_pct: f64,
    pub use_pressure_level: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SwapConfig {
    pub warn_used_gb: f64,
    pub crit_used_gb: f64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            refresh_secs: 5,
            culprits_refresh_secs: 10,
            culprits_count: 8,
            hysteresis_margin_pct: 5.0,
            disk: DiskConfig::default(),
            memory: MemoryConfig::default(),
            swap: SwapConfig::default(),
        }
    }
}

impl Default for DiskConfig {
    fn default() -> Self {
        DiskConfig { warn_free_gb: 25.0, crit_free_gb: 10.0 }
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        MemoryConfig { warn_available_pct: 15.0, crit_available_pct: 7.0, use_pressure_level: true }
    }
}

impl Default for SwapConfig {
    fn default() -> Self {
        SwapConfig { warn_used_gb: 4.0, crit_used_gb: 10.0 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    Io(String),
    Parse(String),
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(m) | ConfigError::Parse(m) | ConfigError::Invalid(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Parse a TOML string into a validated `Config`.
pub fn parse(s: &str) -> Result<Config, ConfigError> {
    let c: Config = toml::from_str(s).map_err(|e| ConfigError::Parse(e.to_string()))?;
    validate(&c)?;
    Ok(c)
}

fn invalid(msg: &str) -> Result<(), ConfigError> {
    Err(ConfigError::Invalid(msg.to_string()))
}

pub fn validate(c: &Config) -> Result<(), ConfigError> {
    if c.refresh_secs < 1 {
        return invalid("refresh_secs must be >= 1");
    }
    if c.culprits_refresh_secs < 1 {
        return invalid("culprits_refresh_secs must be >= 1");
    }
    if !(1..=30).contains(&c.culprits_count) {
        return invalid("culprits_count must be between 1 and 30");
    }
    if !(0.0..=50.0).contains(&c.hysteresis_margin_pct) {
        return invalid("hysteresis_margin_pct must be between 0 and 50");
    }
    let d = &c.disk;
    if !(0.0 < d.crit_free_gb && d.crit_free_gb < d.warn_free_gb) {
        return invalid("[disk] requires 0 < crit_free_gb < warn_free_gb");
    }
    let m = &c.memory;
    if !(0.0 < m.crit_available_pct && m.crit_available_pct < m.warn_available_pct && m.warn_available_pct < 100.0) {
        return invalid("[memory] requires 0 < crit_available_pct < warn_available_pct < 100");
    }
    let s = &c.swap;
    if !(0.0 < s.warn_used_gb && s.warn_used_gb < s.crit_used_gb) {
        return invalid("[swap] requires 0 < warn_used_gb < crit_used_gb");
    }
    Ok(())
}

/// The commented config file written on first run.
pub fn default_toml() -> &'static str {
    r#"# mac-health-monitor configuration
# Disk values use decimal GB (1 GB = 1,000,000,000 bytes).
# Memory and swap values use binary GiB (1,073,741,824 bytes), shown as "GB" in the menu.

# How often the stats are sampled, in seconds (>= 1).
refresh_secs = 5

# How often the top-culprit process list is refreshed, in seconds (>= 1).
culprits_refresh_secs = 10

# Number of top memory-consuming apps shown in the menu (1-30).
culprits_count = 8

# Hysteresis: an improving status must pass its threshold by this margin,
# as a percentage of the threshold value, for 2 samples in a row (0-50).
hysteresis_margin_pct = 5.0

[disk]
# Free space on the boot volume, in decimal GB. Yellow below warn, red below crit.
warn_free_gb = 25
crit_free_gb = 10

[memory]
# Available memory as a percentage of total RAM (binary GiB based). Yellow below warn, red below crit.
warn_available_pct = 15
crit_available_pct = 7
# Also use the kernel memory-pressure level (warn / critical) to raise the status.
use_pressure_level = true

[swap]
# Swap in use, in binary GiB. Yellow above warn, red above crit.
warn_used_gb = 4
crit_used_gb = 10
"#
}

/// `$HOME/.config/mac-health-monitor/config.toml`.
pub fn config_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".config").join("mac-health-monitor").join("config.toml")
}

/// Load the config at `path`, creating it with commented defaults when missing.
pub fn load_or_create(path: &Path) -> Result<Config, ConfigError> {
    let io = |e: std::io::Error| ConfigError::Io(format!("{}: {}", path.display(), e));
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        std::fs::write(path, default_toml()).map_err(io)?;
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(path).map_err(io)?;
    parse(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn defaults_match_spec() {
        let c = Config::default();
        assert_eq!((c.refresh_secs, c.culprits_refresh_secs, c.culprits_count), (5, 10, 8));
        assert_eq!(c.hysteresis_margin_pct, 5.0);
        assert_eq!((c.disk.warn_free_gb, c.disk.crit_free_gb), (25.0, 10.0));
        assert_eq!((c.memory.warn_available_pct, c.memory.crit_available_pct, c.memory.use_pressure_level), (15.0, 7.0, true));
        assert_eq!((c.swap.warn_used_gb, c.swap.crit_used_gb), (4.0, 10.0));
    }
    #[test] fn default_toml_parses_to_default() { assert_eq!(parse(default_toml()).unwrap(), Config::default()); }
    #[test] fn partial_file_fills_defaults() {
        let c = parse("[swap]\nwarn_used_gb = 6\n").unwrap();
        assert_eq!(c.swap.warn_used_gb, 6.0); assert_eq!(c.swap.crit_used_gb, 10.0); assert_eq!(c.refresh_secs, 5);
    }
    #[test] fn parse_rejects_unknown_key() { assert!(matches!(parse("refresh_sec = 5"), Err(ConfigError::Parse(_)))); }
    #[test] fn parse_rejects_crit_above_warn() {
        let e = parse("[disk]\nwarn_free_gb = 10\ncrit_free_gb = 20\n").unwrap_err();
        assert!(matches!(&e, ConfigError::Invalid(m) if m.contains("disk")));
    }
    #[test] fn parse_rejects_zero_refresh() { assert!(matches!(parse("refresh_secs = 0"), Err(ConfigError::Invalid(_)))); }
    #[test] fn load_or_create_writes_defaults() {
        let dir = std::env::temp_dir().join(format!("mhm-test-{}", std::process::id()));
        let p = dir.join("nested/config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load_or_create(&p).unwrap(), Config::default());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), default_toml());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
