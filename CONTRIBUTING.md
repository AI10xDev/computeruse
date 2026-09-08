# Contributing

Use Rust 1.88 or newer and keep changes focused on Linux input behavior.

Before opening a pull request, run:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo doc --no-deps
```

Tests should use a fake implementation of `MouseBackend` unless they are
explicitly marked as hardware integration tests. Never assume CI can read
`/dev/input` or write `/dev/uinput`.

Report security-sensitive input handling issues privately to the repository
maintainers instead of publishing exploit details in an issue.
