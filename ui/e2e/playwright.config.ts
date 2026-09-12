import { defineConfig, devices } from "@playwright/test";

/**
 * Web-mode e2e harness (no tauri-driver yet).
 *
 * Playwright drives the Vite dev server directly; the Tauri backend is
 * replaced by a canned `__TAURI_INTERNALS__` mock installed via
 * `page.addInitScript` (see `./tauri-mock.ts`). Nothing here touches a real
 * WebView, so these specs run on any machine with a browser.
 *
 * Run: `bun run test:e2e` (root) — deliberately NOT part of the default
 * `bun run test`, so unit runs (`bun test` / `cargo test`) stay fast.
 */
export default defineConfig({
  testDir: "./",
  testMatch: "**/*.spec.ts",
  fullyParallel: true,
  retries: process.env.CI ? 2 : 0,
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:1420",
    trace: "on-first-retry",
  },
  webServer: {
    // Pin IPv4 loopback: vite otherwise binds ::1 only on some machines and
    // the `url` poll below (plus baseURL) targets 127.0.0.1 explicitly.
    command: "bun run dev -- --host 127.0.0.1 --port 1420 --strictPort",
    url: "http://127.0.0.1:1420",
    reuseExistingServer: !process.env.CI,
    timeout: 120 * 1000,
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
