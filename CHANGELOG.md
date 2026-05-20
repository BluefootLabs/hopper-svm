# Changelog

## Unreleased

- Bumped the standalone crate, FFI crate, and language binding package metadata
  to `0.1.1`.
- Raised the Solana and Agave crate floor to the `2.3` series used by current
  Hopper validation.
- Refreshed docs to make `agave-runtime` the canonical mainnet-fidelity path.
- Reworked `bpf-execution` so the old BPF registration API now loads and runs
  programs through Agave's Solana runtime path.
- Split `hopper-svm` out of the main Hopper framework repository as a standalone
  test-harness product.
- Added `hopper-svm-ffi` as an optional workspace crate for host-language
  bindings.
