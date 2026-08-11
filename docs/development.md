# Development

```console
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo check -p varchar --target wasm32-unknown-unknown
cargo check -p varchar --target wasm32-wasip1
wasm-pack test --node crates/varchar
```

Unit tests live in dedicated child-module `tests.rs` files; implementation
modules contain only the `#[cfg(test)] mod tests;` declaration. Public and
cross-module behavior remains covered by the integration suites under each
crate's top-level `tests/` directory.

CI runs the same native and WebAssembly gates.
