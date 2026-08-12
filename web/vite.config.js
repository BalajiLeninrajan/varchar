import { defineConfig } from "vite";
import preact from "@preact/preset-vite";

export default defineConfig({
  plugins: [preact()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    // The engine is one large .wasm asset; inlining anything near it would be
    // worse than a second request.
    assetsInlineLimit: 4096,
  },
});
