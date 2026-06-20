# Changelog

All notable changes to `buzz-tui` will be documented in this file.

## 0.1.0 - 2026-07-25

- Extracted the client-backed Ratatui application from the Buzz monorepo.
- Pinned all Buzz protocol and transport crates to one exact Git revision.
- Added standalone Cargo, Nix, CI, and release-bundle configuration.
- Release bundles include compatible `buzz-acp` and `buzz-dev-mcp` sidecars
  built from the same Buzz revision.

