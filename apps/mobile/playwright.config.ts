import { defineConfig } from '@playwright/test'
const port = process.env.OB_E2E_UI_PORT || '48173'
export default defineConfig({
  testDir: './e2e',
  fullyParallel: false,
  workers: 1,
  timeout: 45000,
  expect: { timeout: 10000 },
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    viewport: { width: 1366, height: 900 },
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure'
  },
  webServer: {
    command: 'node e2e/server.mjs',
    url: `http://127.0.0.1:${port}/network`,
    timeout: 30000,
    reuseExistingServer: false,
    gracefulShutdown: { signal: 'SIGTERM', timeout: 5000 }
  }
})
