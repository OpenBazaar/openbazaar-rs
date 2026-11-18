import { defineConfig } from '@playwright/test'
// Tests against real disposable nodes (e2e/nodes.mjs). Each test starts its own
// nodes on free ports, so nothing is shared between tests.
export default defineConfig({
  fullyParallel: true,
  workers: process.env.CI ? 2 : 3,
  retries: 0,
  timeout: 60000,
  expect: { timeout: 10000 },
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }], ['github']] : [['list'], ['html', { open: 'never' }]],
  projects: [{ name: 'real', testDir: './e2e/real' }],
  use: {
    actionTimeout: 15000,
    viewport: { width: 1366, height: 900 },
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure'
  }
})
