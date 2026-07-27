# Contributing to buzz-tui

`buzz-tui` is an independent Rust project whose Buzz protocol dependencies are
pinned to one exact commit in `Cargo.toml`.

Before submitting a change, use Rust 1.88 or newer and run:

```bash
cargo fmt --check
cargo test --locked
cargo check --locked --no-default-features
cargo clippy --locked --all-targets --all-features -- -D warnings
```

Keep UI state, feature filters, parsing, and process supervision in this
project. Generic authenticated relay transport belongs in `buzz-client` in the
main Buzz repository. When updating Buzz dependencies, update every Buzz Git
dependency to the same full commit ID, regenerate `Cargo.lock`, and verify both
default and headless feature sets.

Please report bugs and propose changes through the
[buzz-tui issue tracker](https://github.com/block/buzz-tui/issues).

