# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.1] - 2026-11-23

### Added

- "About Mac Health Monitor" menu item showing the version and a short description of the app

## [0.2.0] - 2026-11-20

### Added

- Configuration for the history recorder and health report (`[history]`, `[report]`)
- Local-time formatting and boot-time lookup for reports
- History recorder: one sample per minute to `~/Library/Logs/mac-health-monitor/history.jsonl`, kept 24 h, synced to disk so it survives a crash
- Parsers for macOS kernel panic, watchdog, memory-pressure (Jetsam) and reset reports
- Discovery of recent incident reports in `/Library/Logs/DiagnosticReports`; unreadable files become notes, never errors
- Plain-language findings ("What this suggests") derived from incidents, current state and history
- Markdown health report: findings, incidents, history timeline, current state and sources
- Health report generation (the newest `keep_reports`, default 20, are kept) and detection of crash-caused reboots
- "Health report…" menu item (generated in the background and opened), "⚠ Crashed at …" line after a crash-caused reboot, and history recording in the running app
- Crash detection runs in the background and is re-checked once a minute for about 20 minutes after boot, so the "⚠ Crashed at …" line still appears when macOS finishes writing the panic report after the app has started at login
- The report's history timeline looks back from the newest crash (kernel panic or hang-reset), else the newest memory-pressure kills, and names it in its heading; when that hour has no history, the last 60 minutes are shown as well

## [0.1.1] - 2026-11-07

### Added

- MIT license file

### Fixed

- Colored status dots now actually appear next to each parameter in the menu. Recent macOS releases do not draw menu item images, so the dot is now part of the row text, where it keeps full color on the display-only rows

## [0.1.0] - 2026-11-06

### Added

- Status model and menu-bar icon selection
- TOML configuration with validation and commented defaults
- System metrics sampling for disk, memory, pressure and swap
- Threshold evaluation with hysteresis for disk, memory and swap
- Per-process memory sampling grouped into top culprits by app
- Menu model with formatted values for parameter and culprit rows
- Monitor state machine tying sampling, evaluation, throttled culprits and the menu together
- Menu-bar status item with colored health icon and a live-updating dropdown menu

### Fixed

- Culprit labels for CLIs installed under paths containing spaces (no more bogus "Application" group); login-shell dash and process-title colon are trimmed
- `make install` no longer races `launchctl bootout` against `bootstrap` on reinstall
- LaunchAgent restarts the app after a crash but stays quit after Quit
