# WebAssembly

The core is kept compatible with both `wasm32-unknown-unknown` and `wasm32-wasip1`. It avoids native libraries, ambient filesystem access, networking, randomness, and threads, and applies configured limits to inputs, generated patterns, logical `SELECT` working/output charges, storage reconstruction, mutation planning, join execution, and regex backtracking.

No JavaScript/WASM package is published. The browser adapter that the playground runs on lives in [`web/wasm`](../web/wasm) and is deliberately outside the Cargo workspace, so the published crates and their CI gates stay unaffected by it. It keeps one `Database` in WebAssembly memory, executes one statement per call, and hands the page the complete blob back; the page owns whatever persistence it wants, which in the playground's case is none. See it at [varchar.balajileninrajan.dev](https://varchar.balajileninrajan.dev/).

A future WASI adapter can provide capability-based persistence separately.

