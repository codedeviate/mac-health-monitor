# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- MIT license file

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
