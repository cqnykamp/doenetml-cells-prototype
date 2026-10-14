import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { resolve } from "node:path";

// The DoenetML parser and static assets are imported from a sibling
// checkout until this package lives in that repo (see
// ../cells-core/scripts/parse-dast.mjs). Set DOENETML_DIR to point at it.
const doenetml = process.env.DOENETML_DIR ?? resolve(__dirname, "../../../../ml");
const repoRoot = resolve(__dirname, "../..");
const core = resolve(__dirname, "../cells-core");

export default defineConfig({
  plugins: [react()],
  // The generated fixtures, served at /<spec>.cdast and /<spec>.json.
  publicDir: resolve(core, "fixtures"),
  resolve: {
    alias: {
      "@doenet/parser": resolve(doenetml, "packages/parser/dist/index.js"),
      "@doenet/static-assets/schema": resolve(doenetml, "packages/static-assets/dist/schema.js"),
      "@doenet/static-assets/entity-map": resolve(doenetml, "packages/static-assets/dist/entity-map.js"),
    },
  },
  // The core is a workspace package whose wasm must not be pre-bundled.
  optimizeDeps: { exclude: ["@doenet/cells-core"] },
  server: {
    port: 5173,
    fs: { allow: [repoRoot, doenetml] },
  },
  build: { target: "esnext" },
});
