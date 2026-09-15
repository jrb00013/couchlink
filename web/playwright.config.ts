import { defineConfig, devices } from "@playwright/test";

// Headless Chromium only: WebCodecs / VideoDecoder is a Chromium feature,
// there is no reason to also run this against Firefox/WebKit here.
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: [["list"]],
  use: {
    baseURL: "http://127.0.0.1:5183",
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  webServer: {
    // Fixed port distinct from the normal dev server (5174) so this suite
    // never collides with a `npm run dev` already running locally.
    command: "npx vite --port 5183 --strictPort",
    url: "http://127.0.0.1:5183",
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
});
