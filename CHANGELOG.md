# Changelog

## Unreleased

### Added

- Prompt steps can declare ordered `quality_gates` commands that verify successful work and retry the step with sanitized failure feedback within the existing edge budget.

### Fixed

- Retain quality-gate output when sanitizing large streams runs past the post-exit pipe-drain grace.
