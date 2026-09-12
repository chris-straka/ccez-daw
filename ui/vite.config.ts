import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// Solid-in-Tauri is a community Vite-plugin path: the Solid plugin compiles
// fine-grained components to real DOM, and Tauri serves this bundle either
// from the dev server (devUrl) or from frontendDist (release build).
export default defineConfig({
  plugins: [solid()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "es2022",
  },
});
