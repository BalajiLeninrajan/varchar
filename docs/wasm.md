# WebAssembly

The core is kept compatible with both `wasm32-unknown-unknown` and `wasm32-wasip1`. It avoids native libraries, ambient filesystem access, networking, randomness, and threads, and applies configured limits to inputs, generated patterns, logical `SELECT` working/output charges, storage reconstruction, mutation planning, join execution, and regex backtracking.

There is no public JavaScript/WASM package in v1. A future browser adapter can pass the complete blob into the same core, execute one statement per call, and persist the returned blob in a browser-owned store. A future WASI adapter can provide capability-based persistence separately.

