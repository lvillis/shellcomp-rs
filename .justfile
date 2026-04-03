set shell := ["bash", "-euo", "pipefail", "-c"]

ci:
  cargo fmt --all --check
  cargo check --all-targets --all-features
  cargo rustc --lib --all-features -- -D missing-docs
  cargo clippy --all-targets --all-features -- -D warnings
  cargo test --doc --all-features
  cargo test --all-features
  cargo doc --no-deps --all-features
  RUSTDOCFLAGS='--cfg docsrs' cargo +nightly doc --all-features --no-deps
  cargo publish --dry-run --allow-dirty

patch:
  cargo release patch --no-publish --execute
