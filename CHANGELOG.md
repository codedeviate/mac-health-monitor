# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Status model and menu-bar icon selection
- TOML configuration with validation and commented defaults
- System metrics sampling for disk, memory, pressure and swap
- Threshold evaluation with hysteresis for disk, memory and swap
- Per-process memory sampling grouped into top culprits by app
- Menu model with formatted values for parameter and culprit rows
- Monitor state machine tying sampling, evaluation, throttled culprits and the menu together
