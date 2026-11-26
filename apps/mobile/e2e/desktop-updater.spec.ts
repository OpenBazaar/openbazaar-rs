import { test, expect } from '@playwright/test'
import { mkdirSync } from 'node:fs'
import { MB, setupMockUpdater } from './updater-mock'
import type { UpdateStatus } from '../src/updater'

const PREVIEW_DIR = '/tmp/ob-updater-preview'

test('available update -> downloading (determinate bar) -> cancel -> retry -> installing', async ({ page }) => {
  await setupMockUpdater(page, {
    stage: 'available',
    available_version: '0.2.0',
    release_notes: '• Faster search over Tor\n• Direct-order reliability fixes'
  })

  await page.goto('/')
  await expect(page.getByRole('heading', { name: /^All products/ })).toBeVisible()

  // 1. Available banner appears with release notes and Download & install
  const banner = page.locator('.ob-update-banner')
  await expect(banner).toBeVisible()
  await expect(banner.locator('strong')).toContainText('OpenBazaar 0.2.0 is ready')

  // Sidebar pill says "Update ready"
  const pill = page.locator('.ob-update-pill')
  await expect(pill).toBeVisible()
  await expect(pill).toContainText('Update ready')

  // Expand release notes
  const notesDetails = banner.locator('details.ob-update-notes')
  await expect(notesDetails).toBeVisible()
  await notesDetails.locator('summary').click()
  await expect(notesDetails.locator('p')).toContainText('Faster search over Tor')

  // 2. Click "Download & install" -> transitions to downloading
  const installButton = banner.getByRole('button', { name: 'Download & install' })
  await installButton.click()

  // Progress bar renders determinate progress (42%)
  const progressBar = banner.getByRole('progressbar', { name: 'Downloading OpenBazaar 0.2.0' })
  await expect(progressBar).toBeVisible()
  await expect(progressBar).toHaveAttribute('aria-valuenow', '42')
  await expect(progressBar).toHaveAttribute('aria-valuetext', '42%')

  // Banner details render percentage, MB transferred, and speed over Tor
  await expect(banner.locator('.ob-update-detail')).toContainText('42%')
  await expect(banner.locator('.ob-update-detail')).toContainText('18.4 / 43.8 MB · 1.2 MB/s over Tor')

  // Sidebar pill reflects live download percentage
  await expect(pill).toContainText('Downloading 42%')
  await expect(pill.locator('.ob-status-dot')).toHaveClass(/is-loading/)

  // 3. Click "Cancel" -> returns cleanly to available state
  const cancelButton = banner.getByRole('button', { name: 'Cancel' })
  await cancelButton.click()

  await expect(banner.locator('strong')).toContainText('OpenBazaar 0.2.0 is ready')
  await expect(banner.getByRole('button', { name: 'Download & install' })).toBeVisible()
  await expect(pill).toContainText('Update ready')

  // 4. Click "Download & install" again -> transitions to installing state
  await page.evaluate(() => {
    const mock = (window as unknown as { __MOCK_SHELL__: { setNextInstallStatus: (s: unknown) => void } }).__MOCK_SHELL__
    mock.setNextInstallStatus({
      stage: 'installing',
      current_version: '0.1.0-alpha.1',
      available_version: '0.2.0',
      release_notes: null,
      downloaded_bytes: 43.8 * 1024 * 1024,
      total_bytes: 43.8 * 1024 * 1024,
      bytes_per_sec: null,
      checked_at: null,
      error: null,
      automatic: true,
      location_problem: null,
      can_check: true
    })
  })

  await banner.getByRole('button', { name: 'Download & install' }).click()

  // Installing state banner is non-dismissible and has indeterminate progress bar
  await expect(banner.locator('strong')).toContainText('Verifying & installing OpenBazaar 0.2.0…')
  await expect(banner.locator('.ob-update-detail')).toContainText('OpenBazaar will restart on its own when this is done.')
  const installingProgress = banner.getByRole('progressbar')
  await expect(installingProgress).toHaveClass(/is-indeterminate/)
  await expect(installingProgress).not.toHaveAttribute('aria-valuenow')

  // No buttons in installing state (non-dismissible)
  await expect(banner.locator('button')).toHaveCount(0)
})

test('Settings "App updates" card, automatic updates toggle, check for updates, error retry, and location warning', async ({ page }) => {
  await setupMockUpdater(page, {
    stage: 'available',
    available_version: '0.2.0',
    release_notes: '• Security updates\n• Bug fixes'
  })

  await page.goto('/')
  await expect(page.getByRole('heading', { name: /^All products/ })).toBeVisible()

  // 1. Clicking sidebar pill opens Settings › App updates
  await page.locator('.ob-update-pill').click()
  await expect(page.getByRole('heading', { name: 'App updates', exact: true })).toBeVisible()
  await expect(page.getByRole('heading', { name: 'Version 0.1.0-alpha.1', exact: true })).toBeVisible()

  // 1b. Clicking sidebar "App updates" link in Settings menu also opens App updates
  await page.getByRole('button', { name: 'Products', exact: true }).click()
  await expect(page.getByRole('heading', { name: /^All products/ })).toBeVisible()
  const settingsToggle = page.getByRole('complementary', { name: 'Main navigation', exact: true }).getByRole('button', { name: 'Settings', exact: true })
  await settingsToggle.click()
  const appUpdatesLink = page.getByRole('button', { name: 'App updates', exact: true })
  await expect(appUpdatesLink).toBeVisible()
  await appUpdatesLink.click()
  await expect(page.getByRole('heading', { name: 'App updates', exact: true })).toBeVisible()

  const card = page.locator('.ob-update-card')
  await expect(card).toBeVisible()
  await expect(card.locator('.ob-update-summary')).toContainText('Update v0.2.0 available')

  // Release notes are open in Settings
  await expect(card.locator('details.ob-update-notes p')).toContainText('Security updates')

  // 2. Toggle automatic updates calls set_automatic_updates
  const autoCheckbox = card.getByRole('checkbox', { name: 'Check for updates automatically' })
  await expect(autoCheckbox).toBeChecked()

  await autoCheckbox.click()
  await expect(autoCheckbox).not.toBeChecked()

  let calls = await page.evaluate(() => {
    return (window as unknown as { __MOCK_SHELL__: { getCalls: () => Array<{ command: string; args?: Record<string, unknown> }> } }).__MOCK_SHELL__.getCalls()
  })
  expect(calls.some(c => c.command === 'set_automatic_updates' && c.args?.enabled === false)).toBeTruthy()

  await autoCheckbox.click()
  await expect(autoCheckbox).toBeChecked()

  calls = await page.evaluate(() => {
    return (window as unknown as { __MOCK_SHELL__: { getCalls: () => Array<{ command: string; args?: Record<string, unknown> }> } }).__MOCK_SHELL__.getCalls()
  })
  expect(calls.some(c => c.command === 'set_automatic_updates' && c.args?.enabled === true)).toBeTruthy()

  // 3. Click "Check for updates" -> transitions checking -> up_to_date
  const checkButton = card.getByRole('button', { name: 'Check for updates' })
  await checkButton.click()
  await expect(card.locator('.ob-update-summary')).toContainText('Checking over Tor…')
  await expect(card.locator('.ob-update-summary')).toContainText('Up to date')

  // 4. Error state displays plain error + Retry button
  await page.evaluate(() => {
    const mock = (window as unknown as { __MOCK_SHELL__: { setStatus: (p: Record<string, unknown>) => void } }).__MOCK_SHELL__
    mock.setStatus({
      stage: 'error',
      available_version: '0.2.0',
      error: 'Tor circuit timed out while fetching update manifest'
    })
  })

  await expect(card.locator('.ob-update-summary')).toContainText('Tor circuit timed out while fetching update manifest')
  const retryButton = card.getByRole('button', { name: 'Retry' })
  await expect(retryButton).toBeVisible()

  // Clicking Retry calls install when available_version is known
  await retryButton.click()
  await expect(card.locator('.ob-update-summary')).toContainText('Downloading v0.2.0…')

  // 5. location_problem warning blocks install cleanly with instructions
  await page.evaluate(() => {
    const mock = (window as unknown as { __MOCK_SHELL__: { setStatus: (p: Record<string, unknown>) => void } }).__MOCK_SHELL__
    mock.setStatus({
      stage: 'available',
      available_version: '0.2.0',
      location_problem: 'Move OpenBazaar to /Applications to enable automatic updates.'
    })
  })

  await expect(card.locator('.ob-update-warning')).toContainText('Move OpenBazaar to /Applications to enable automatic updates.')
  await expect(card.getByRole('button', { name: 'Download & install' })).toHaveCount(0)

  // Top banner is hidden while on the Updates settings page to avoid duplicate banners
  await expect(page.locator('.ob-update-banner')).toHaveCount(0)

  // Navigating to Marketplace shows the top banner with the location warning
  await page.getByRole('button', { name: 'Products', exact: true }).click()
  const banner = page.locator('.ob-update-banner')
  await expect(banner).toBeVisible()
  await expect(banner.getByText('Move OpenBazaar to /Applications to enable automatic updates.')).toBeVisible()
  await expect(banner.getByRole('button', { name: 'Download & install' })).toHaveCount(0)
  await expect(banner.getByRole('button', { name: 'Later' })).toBeVisible()
})

test('320px mobile viewport & dark mode: zero horizontal overflow with active downloading banner and settings card', async ({ page }) => {
  await page.setViewportSize({ width: 320, height: 700 })
  await setupMockUpdater(page, {
    stage: 'downloading',
    available_version: '0.2.0',
    downloaded_bytes: Math.round(18.4 * MB),
    total_bytes: Math.round(43.8 * MB),
    bytes_per_sec: Math.round(1.2 * MB),
    release_notes: '• Mobile responsive improvements'
  })

  await page.goto('/')
  await expect(page.locator('.ob-update-banner')).toBeVisible()

  // Zero overflow on main browse view with downloading banner
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= 320)).toBeTruthy()

  // Open mobile drawer and navigate to App updates
  await page.getByRole('button', { name: 'Open navigation', exact: true }).click()
  await page.locator('.ob-update-pill').click()

  await expect(page.getByRole('heading', { name: 'App updates', exact: true })).toBeVisible()
  const card = page.locator('.ob-update-card')
  await expect(card).toBeVisible()
  await expect(page.locator('.ob-update-banner')).toHaveCount(0)

  // Zero overflow on App updates settings page
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= 320)).toBeTruthy()

  const cardBounds = await card.boundingBox()
  expect(cardBounds!.x).toBeGreaterThanOrEqual(0)
  expect(cardBounds!.x + cardBounds!.width).toBeLessThanOrEqual(320)

  // Verify in dark mode as well
  await page.evaluate(() => {
    localStorage.setItem('ob-theme', 'dark')
    document.documentElement.dataset.obTheme = 'dark'
  })

  expect(await page.evaluate(() => document.documentElement.scrollWidth <= 320)).toBeTruthy()
})

test('non-desktop browser mode: without __TAURI_INTERNALS__, banner and settings nav item are hidden', async ({ page }) => {
  // Do NOT setup mock updater: standard browser environment
  await page.goto('/')
  await expect(page.getByRole('heading', { name: /^All products/ })).toBeVisible()

  // No update banner
  await expect(page.locator('.ob-update-banner')).toHaveCount(0)

  // No sidebar pill
  await expect(page.locator('.ob-update-pill')).toHaveCount(0)

  // Open settings menu: no "App updates" item
  const settingsToggle = page.getByRole('complementary', { name: 'Main navigation', exact: true }).getByRole('button', { name: 'Settings', exact: true })
  await settingsToggle.click()
  await expect(page.getByRole('button', { name: 'App updates', exact: true })).toHaveCount(0)
})

test.describe('visual verification screenshots', () => {
  test.use({ deviceScaleFactor: 2 })

  test('captures preview screenshots in light, dark, and mobile viewports', async ({ page }) => {
    mkdirSync(PREVIEW_DIR, { recursive: true })

    // 1. Desktop downloading banner in light mode
    await page.setViewportSize({ width: 1366, height: 900 })
    await setupMockUpdater(page, {
      stage: 'downloading',
      available_version: '0.2.0',
      downloaded_bytes: Math.round(18.4 * MB),
      total_bytes: Math.round(43.8 * MB),
      bytes_per_sec: Math.round(1.2 * MB),
      release_notes: '• Faster search over Tor\n• Direct-order reliability fixes'
    })

    await page.addInitScript(() => {
      localStorage.setItem('ob-theme', 'light')
      document.documentElement.dataset.obTheme = 'light'
    })

    await page.goto('/')
    await expect(page.getByRole('heading', { name: /^All products/ })).toBeVisible()
    await expect(page.locator('.ob-update-banner')).toBeVisible()
    await expect(page.locator('.ob-update-pill')).toBeVisible()

    await page.screenshot({
      path: `${PREVIEW_DIR}/01-downloading-banner-light.png`,
      fullPage: false
    })

    // 2. Desktop Updates settings in dark mode with progress bar and release notes
    await page.evaluate(() => {
      localStorage.setItem('ob-theme', 'dark')
      document.documentElement.dataset.obTheme = 'dark'
    })

    await page.locator('.ob-update-pill').click()
    await expect(page.getByRole('heading', { name: 'App updates', exact: true })).toBeVisible()
    await expect(page.locator('.ob-update-card')).toBeVisible()
    await expect(page.locator('.ob-update-card details.ob-update-notes')).toBeVisible()
    await expect(page.locator('.ob-update-banner')).toHaveCount(0)

    await page.screenshot({
      path: `${PREVIEW_DIR}/02-updates-settings-dark.png`,
      fullPage: false
    })

    // 3. Mobile 320x700 downloading banner with zero overflow
    await page.setViewportSize({ width: 320, height: 700 })
    await page.evaluate(() => {
      localStorage.setItem('ob-theme', 'light')
      document.documentElement.dataset.obTheme = 'light'
    })

    await page.goto('/')
    await expect(page.locator('.ob-update-banner')).toBeVisible()
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= 320)).toBeTruthy()

    await page.screenshot({
      path: `${PREVIEW_DIR}/03-mobile-320-downloading.png`,
      fullPage: false
    })
  })
})
