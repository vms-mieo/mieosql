import path from "node:path";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const root = import.meta.dirname;

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      // TypeScript contract generated from the Rust backend
      // (`cargo run -p mieosql-api --bin export-ts -- apps/mieosql-desktop/frontend/src/bindings`).
      "@bindings": path.resolve(root, "src/bindings"),
    },
  },
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    fs: { allow: [path.resolve(root, "../..")] },
  },
  build: {
    outDir: "dist",
    target: "es2022",
    emptyOutDir: true,
  },
});
