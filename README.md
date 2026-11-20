# mac-health-monitor

A macOS menu-bar app that watches disk space, memory and swap and shows a single
colored status icon, so you can see at a glance whether your Mac is running low on
something. Click the icon for the numbers and the apps most likely responsible.

It is a menu-bar-only agent: no Dock icon and no windows. Requires macOS 13 or later.

The app is light. Its own memory is about 14 MB physical footprint (the figure
Activity Monitor shows). `ps` reports an RSS of around 70 MB because it also counts
AppKit's shared system libraries, which are not really the app's own memory.

## What it shows

Each parameter is green (OK), yellow (warning) or red (critical). The icon reflects
the overall state:

| Situation | Menu-bar icon |
|---|---|
| All parameters OK | green thumbs-up |
| Only memory not OK | memory chip, yellow or red |
| Only disk not OK | hard drive, yellow or red |
| Only swap not OK | recycle symbol, yellow or red |
| Two or more parameters not OK | warning triangle, colored by the worst status |

Clicking the icon opens a menu with:

- each parameter with a colored dot and its current value: disk free, memory
  available, memory pressure, swap used and compressed memory
- a "Potential culprits" section with the top memory consumers, grouped by app
  (for example `Arc Helper (Renderer)` counts toward `Arc`)
- Health report... (see below)
- Open config..., Reload config and Quit

Memory pressure and compressed memory are informational and do not affect the icon.

## Health report

"Health report..." in the menu builds a Markdown report in the background and opens
it. It answers "why did my Mac misbehave or restart?" and contains:

- **What this suggests**: plain-language, hedged findings (for example that memory
  ran out before a crash, that swap has stayed high, or that a restart may help),
  each citing the numbers behind it
- **Incidents** from the last `incident_days` days: kernel panics, memory-pressure
  (jetsam) events, watchdog timeouts and resets
- **A timeline** of disk, memory and swap from the recorded history
- **Now**: the current readings and top memory consumers
- **Sources**: which files were read, and any that could not be parsed

While it runs, the app records one small sample of its readings per minute so the
report can show what led up to an incident.

**Where things are stored.** History and reports live in
`~/Library/Logs/mac-health-monitor/` (`history.jsonl` and `reports/`). The newest
`keep_reports` reports are kept.

**Crash line.** After a reboot caused by a crash, the menu shows a
"⚠ Crashed at ..." line. It disappears once you open a health report, or after
`incident_days` days.

**What it reads.** The app only reads the macOS diagnostics folder
`/Library/Logs/DiagnosticReports` (and its `Retired` subfolder). It never runs
`log`, `sysdiagnose` or `spindump`. A missing or unreadable file never stops the
report; it becomes a note under Sources.

## Thresholds and units

| Parameter | Yellow | Red |
|---|---|---|
| Disk free | below 25 GB | below 10 GB |
| Memory | available below 15%, or kernel pressure is warning | available below 7%, or kernel pressure is critical |
| Swap used | above 4 GB | above 10 GB |

Comparisons are strict, so a value exactly at a threshold is not counted as worse.

**Units.** Disk uses decimal GB (1 GB = 1,000,000,000 bytes), matching Finder and
`df -H`. Memory and swap use binary GiB (1,073,741,824 bytes), matching Activity
Monitor, but are labelled "GB" in the menu. Config values are interpreted the same
way.

**Hysteresis.** Getting worse is immediate. Getting better is not: a parameter only
steps down in severity (for example yellow to green) after it has been on the better
side of the threshold by a margin of `hysteresis_margin_pct` (default 5% of the
threshold value) for 2 consecutive samples. This stops the icon flickering around a
threshold.

## Configuration

The config file is `~/.config/mac-health-monitor/config.toml`. If it does not exist
at start, it is created with commented defaults. Use "Open config..." in the menu to
edit it and "Reload config" to apply changes. If the file is invalid, the previous
config is kept and the menu shows a config error line.

| Key | Default | Meaning |
|---|---|---|
| `refresh_secs` | 5 | How often stats are sampled, in seconds (>= 1) |
| `culprits_refresh_secs` | 10 | How often the culprit list is refreshed, in seconds (>= 1) |
| `culprits_count` | 8 | Number of top processes shown (1-30) |
| `hysteresis_margin_pct` | 5.0 | Improvement margin, as a percent of the threshold (0-50) |
| `[disk] warn_free_gb` | 25 | Yellow below this free space (decimal GB) |
| `[disk] crit_free_gb` | 10 | Red below this free space (decimal GB) |
| `[memory] warn_available_pct` | 15 | Yellow below this available percentage of RAM |
| `[memory] crit_available_pct` | 7 | Red below this available percentage of RAM |
| `[memory] use_pressure_level` | true | Also use the kernel memory-pressure level |
| `[swap] warn_used_gb` | 4 | Yellow above this swap used (GiB) |
| `[swap] crit_used_gb` | 10 | Red above this swap used (GiB) |
| `[history] interval_secs` | 60 | Seconds between history samples (>= 10) |
| `[history] retain_hours` | 24 | Hours of history kept (1-168) |
| `[report] incident_days` | 7 | Days of incidents a report covers; also how long the crash line can show (1-30) |
| `[report] keep_reports` | 20 | Saved reports kept; older ones are removed (1-500) |

Unknown keys are rejected, and each critical threshold must be on the worse side of
its warning threshold.

## Build and run

Requires a stable Rust toolchain.

```sh
make run      # build the release binary, assemble target/MacHealthMonitor.app and open it
```

Other targets: `make build` (release binary) and `make app` (assemble the bundle).

## Install and uninstall

```sh
make install     # copy the app to ~/Applications and start it at login
make uninstall   # stop it and remove the app and the login agent
```

`make install` copies `MacHealthMonitor.app` to `~/Applications`, writes
`~/Library/LaunchAgents/se.codedeviate.mac-health-monitor.plist` and loads it with
`launchctl`, so the app starts now and at every login. `make uninstall` reverses both.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

- Commits follow [Conventional Commits](https://www.conventionalcommits.org/).
- Every `feat:` or `fix:` commit adds a bullet under `## [Unreleased]` in
  [CHANGELOG.md](CHANGELOG.md), in the same commit.
- The version lives in `Cargo.toml` only; it is substituted into `Info.plist` at build time.

## Out of scope and roadmap

See [OUT-OF-SCOPE.md](OUT-OF-SCOPE.md) for what is deliberately not done and the wishlist.

## License

MIT. See [LICENSE](LICENSE).
