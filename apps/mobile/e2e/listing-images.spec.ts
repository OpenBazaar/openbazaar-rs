import { test, expect, type Page } from '@playwright/test'
import { mkdirSync } from 'node:fs'
import { setupMockUpdater } from './updater-mock'

const PREVIEW_DIR = '/tmp/ob-listing-images-preview'

// Tall portrait trading-card-shaped SVG (250x350) with top/bottom borders and labels.
// Any cropping (e.g. from object-fit: cover) will slice off the top or bottom label.
const PORTRAIT_CARD_SVG = `<svg xmlns="http://www.w3.org/2000/svg" width="250" height="350" viewBox="0 0 250 350">
  <defs>
    <linearGradient id="card-bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0%" stop-color="#1e1b4b"/>
      <stop offset="100%" stop-color="#312e81"/>
    </linearGradient>
  </defs>
  <rect width="250" height="350" rx="14" fill="url(#card-bg)"/>
  <rect x="6" y="6" width="238" height="338" rx="10" fill="none" stroke="#f59e0b" stroke-width="4"/>
  <!-- Top safety label: clipped if object-fit is cover -->
  <rect x="12" y="12" width="226" height="28" rx="5" fill="#f59e0b"/>
  <text x="125" y="31" font-family="-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif" font-size="11" font-weight="700" fill="#78350f" text-anchor="middle">▲ TOP EDGE · NO CROPPING ▲</text>
  <!-- Card artwork -->
  <circle cx="125" cy="150" r="54" fill="#3730a3" stroke="#fbbf24" stroke-width="3"/>
  <polygon points="125,112 140,144 172,144 146,166 156,198 125,177 94,198 104,166 78,144 110,144" fill="#f59e0b"/>
  <text x="125" y="234" font-family="-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif" font-size="16" font-weight="800" fill="#ffffff" text-anchor="middle" letter-spacing="0.5">TRADING CARD #001</text>
  <text x="125" y="256" font-family="-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif" font-size="12" font-weight="600" fill="#c7d2fe" text-anchor="middle">PORTRAIT · 250 × 350</text>
  <text x="125" y="276" font-family="-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif" font-size="11" fill="#a5b4fc" text-anchor="middle">5:7 ASPECT RATIO</text>
  <!-- Bottom safety label: clipped if object-fit is cover -->
  <rect x="12" y="310" width="226" height="28" rx="5" fill="#f59e0b"/>
  <text x="125" y="329" font-family="-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif" font-size="11" font-weight="700" fill="#78350f" text-anchor="middle">▼ BOTTOM EDGE · NO CROPPING ▼</text>
</svg>`

// Wide landscape image (400x200) with left/right borders and labels.
// Any cropping will slice off the left or right edge label.
const LANDSCAPE_SVG = `<svg xmlns="http://www.w3.org/2000/svg" width="400" height="200" viewBox="0 0 400 200">
  <defs>
    <linearGradient id="land-bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0%" stop-color="#064e3b"/>
      <stop offset="100%" stop-color="#047857"/>
    </linearGradient>
  </defs>
  <rect width="400" height="200" rx="12" fill="url(#land-bg)"/>
  <rect x="6" y="6" width="388" height="188" rx="8" fill="none" stroke="#10b981" stroke-width="4"/>
  <!-- Left/right markers: clipped if object-fit is cover -->
  <rect x="12" y="12" width="26" height="176" rx="5" fill="#10b981"/>
  <text x="25" y="105" font-family="-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif" font-size="10" font-weight="700" fill="#064e3b" text-anchor="middle" transform="rotate(-90 25 105)">◄ LEFT EDGE</text>
  <rect x="362" y="12" width="26" height="176" rx="5" fill="#10b981"/>
  <text x="375" y="105" font-family="-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif" font-size="10" font-weight="700" fill="#064e3b" text-anchor="middle" transform="rotate(90 375 105)">RIGHT EDGE ►</text>
  <!-- Content -->
  <circle cx="200" cy="76" r="32" fill="#065f46" stroke="#34d399" stroke-width="2.5"/>
  <text x="200" y="82" font-family="-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif" font-size="20" fill="#6ee7b7" text-anchor="middle">🏔️</text>
  <text x="200" y="132" font-family="-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif" font-size="16" font-weight="800" fill="#ffffff" text-anchor="middle">PANORAMIC PRINT</text>
  <text x="200" y="152" font-family="-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif" font-size="12" fill="#a7f3d0" text-anchor="middle">LANDSCAPE · 400 × 200 (2:1)</text>
</svg>`

// Square image (300x300) with complete perimeter border.
const SQUARE_SVG = `<svg xmlns="http://www.w3.org/2000/svg" width="300" height="300" viewBox="0 0 300 300">
  <defs>
    <linearGradient id="sq-bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0%" stop-color="#4c1d95"/>
      <stop offset="100%" stop-color="#6d28d9"/>
    </linearGradient>
  </defs>
  <rect width="300" height="300" rx="12" fill="url(#sq-bg)"/>
  <rect x="6" y="6" width="288" height="288" rx="8" fill="none" stroke="#a855f7" stroke-width="4"/>
  <rect x="14" y="14" width="272" height="26" rx="4" fill="#a855f7"/>
  <text x="150" y="32" font-family="-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif" font-size="11" font-weight="700" fill="#3b0764" text-anchor="middle">SQUARE FORMAT · 300 × 300</text>
  <circle cx="150" cy="150" r="54" fill="#5b21b6" stroke="#c084fc" stroke-width="3"/>
  <text x="150" y="157" font-family="-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif" font-size="22" font-weight="800" fill="#ffffff" text-anchor="middle">1 : 1</text>
  <rect x="14" y="260" width="272" height="26" rx="4" fill="#a855f7"/>
  <text x="150" y="278" font-family="-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif" font-size="11" font-weight="700" fill="#3b0764" text-anchor="middle">PERFECT SQUARE</text>
</svg>`

async function routeListingImages(page: Page) {
  // Portrait card (250x350) for Paperback novel, Stoneware mug, Wool socks
  await page.route('**/goods/paperback.jpg', route =>
    route.fulfill({ status: 200, contentType: 'image/svg+xml', body: PORTRAIT_CARD_SVG })
  )
  await page.route('**/goods/mug.jpg', route =>
    route.fulfill({ status: 200, contentType: 'image/svg+xml', body: PORTRAIT_CARD_SVG })
  )
  await page.route('**/goods/socks.jpg', route =>
    route.fulfill({ status: 200, contentType: 'image/svg+xml', body: PORTRAIT_CARD_SVG })
  )
  // Landscape (400x200) for Paperback open and Clothbound notebook
  await page.route('**/goods/paperback-open.jpg', route =>
    route.fulfill({ status: 200, contentType: 'image/svg+xml', body: LANDSCAPE_SVG })
  )
  await page.route('**/goods/notebook.jpg', route =>
    route.fulfill({ status: 200, contentType: 'image/svg+xml', body: LANDSCAPE_SVG })
  )
  // Square (300x300) for Work shirt, Coffee, Towel, Honey, Candles, etc.
  for (const imgName of ['shirt', 'coffee', 'towel', 'honey', 'candles', 'trowel', 'cards']) {
    await page.route(`**/goods/${imgName}.jpg`, route =>
      route.fulfill({ status: 200, contentType: 'image/svg+xml', body: SQUARE_SVG })
    )
  }
}

test.describe('listing image aspect-ratio and contain verification', () => {
  test.use({ deviceScaleFactor: 2, viewport: { width: 1366, height: 900 } })

  test.beforeAll(() => {
    mkdirSync(PREVIEW_DIR, { recursive: true })
  })

  test('verifies object-fit: contain on grid, detail, thumbnails and captures visual previews', async ({ page }) => {
    await routeListingImages(page)
    await setupMockUpdater(page, {
      stage: 'available',
      available_version: '0.2.0',
      release_notes: '• Lossless listing image aspect ratios\n• Contain matte presentation'
    })

    // Start in dark mode to verify against the matte stage
    await page.addInitScript(() => {
      localStorage.setItem('ob-theme', 'dark')
      document.documentElement.dataset.obTheme = 'dark'
    })

    await page.goto('/')
    await expect(page.getByRole('heading', { name: /^All products/ })).toBeVisible()

    // Filter to seller Marlow Goods so all items on the grid display actual photos:
    // Row 1: Clothbound notebook (Landscape), Stoneware mug (Square), Work shirt (Square), Wool socks (Portrait)
    await page.locator('details.ob-advanced-filters summary').click()
    await page.getByLabel('Filter by seller').selectOption('Marlow Goods')
    await page.locator('details.ob-advanced-filters summary').click()

    await expect(page.locator('.ob-product-card', { hasText: 'Wool socks' })).toBeVisible()
    await expect(page.locator('.ob-product-card', { hasText: 'Clothbound notebook' })).toBeVisible()

    // 1. Verify Marketplace product grid images
    const gridImages = page.locator('.ob-product-grid .ob-product-image img')
    await expect(gridImages.first()).toBeVisible()

    const imageCount = await gridImages.count()
    expect(imageCount).toBeGreaterThanOrEqual(4)

    // Check all visible product images on grid: object-fit contain, centered, no container overflow
    for (let i = 0; i < imageCount; i++) {
      const img = gridImages.nth(i)
      const metrics = await img.evaluate((el: HTMLImageElement) => {
        const style = window.getComputedStyle(el)
        const container = el.closest('.ob-product-image') as HTMLElement
        const containerRect = container.getBoundingClientRect()
        return {
          objectFit: style.objectFit,
          objectPosition: style.objectPosition,
          containerWidth: containerRect.width,
          containerHeight: containerRect.height,
          scrollWidth: container.scrollWidth,
          clientWidth: container.clientWidth,
          scrollHeight: container.scrollHeight,
          clientHeight: container.clientHeight
        }
      })

      expect(metrics.objectFit).toBe('contain')
      expect(metrics.objectPosition).toMatch(/center|50%/)
      // Container bounds must not be exceeded
      expect(metrics.scrollWidth).toBeLessThanOrEqual(metrics.clientWidth + 1)
      expect(metrics.scrollHeight).toBeLessThanOrEqual(metrics.clientHeight + 1)
    }

    // Capture 01: dark mode Marketplace grid showing portrait card + landscape + square items
    await page.screenshot({
      path: `${PREVIEW_DIR}/01-grid-dark.png`,
      fullPage: false
    })

    // 2. Open Product Detail for "Paperback novel" (portrait trading card hero + gallery thumbnails)
    const paperbackCard = page.locator('.ob-product-card', { hasText: 'Paperback novel' })
    await expect(paperbackCard).toBeVisible()
    await paperbackCard.click()

    await expect(page.getByRole('heading', { name: 'Paperback novel', level: 1 })).toBeVisible()

    // Verify Product Detail hero image
    const heroImage = page.locator('.ob-gallery .ob-product-image img')
    await expect(heroImage).toBeVisible()

    const heroMetrics = await heroImage.evaluate((el: HTMLImageElement) => {
      const style = window.getComputedStyle(el)
      const container = el.closest('.ob-product-image') as HTMLElement
      const containerRect = container.getBoundingClientRect()
      return {
        objectFit: style.objectFit,
        objectPosition: style.objectPosition,
        containerWidth: containerRect.width,
        containerHeight: containerRect.height,
        scrollWidth: container.scrollWidth,
        clientWidth: container.clientWidth,
        scrollHeight: container.scrollHeight,
        clientHeight: container.clientHeight,
        naturalWidth: el.naturalWidth,
        naturalHeight: el.naturalHeight
      }
    })

    expect(heroMetrics.objectFit).toBe('contain')
    expect(heroMetrics.objectPosition).toMatch(/center|50%/)
    expect(heroMetrics.scrollWidth).toBeLessThanOrEqual(heroMetrics.clientWidth + 1)
    expect(heroMetrics.scrollHeight).toBeLessThanOrEqual(heroMetrics.clientHeight + 1)

    // Verify gallery thumbnails
    const thumbnails = page.locator('.ob-thumbnails img')
    await expect(thumbnails.first()).toBeVisible()
    const thumbCount = await thumbnails.count()
    expect(thumbCount).toBeGreaterThanOrEqual(2)

    for (let i = 0; i < thumbCount; i++) {
      const thumb = thumbnails.nth(i)
      const thumbMetrics = await thumb.evaluate((el: HTMLImageElement) => {
        const style = window.getComputedStyle(el)
        return {
          objectFit: style.objectFit,
          objectPosition: style.objectPosition,
          background: style.backgroundColor
        }
      })
      expect(thumbMetrics.objectFit).toBe('contain')
      expect(thumbMetrics.objectPosition).toMatch(/center|50%/)
    }

    // Capture 02: dark mode Product Detail hero & thumbnails showing the full portrait card without cropping
    await page.screenshot({
      path: `${PREVIEW_DIR}/02-detail-dark.png`,
      fullPage: false
    })

    // 3. Return to products and switch to light mode
    await page.getByRole('button', { name: 'Back to products' }).click()
    await expect(page.locator('.ob-product-card', { hasText: 'Wool socks' })).toBeVisible()

    await page.evaluate(() => {
      localStorage.setItem('ob-theme', 'light')
      document.documentElement.dataset.obTheme = 'light'
    })

    // Capture 03: light mode Marketplace grid
    await page.screenshot({
      path: `${PREVIEW_DIR}/03-grid-light.png`,
      fullPage: false
    })

    // 4. Navigate to Settings → App updates and verify NO duplicate top banner
    await page.evaluate(() => {
      localStorage.setItem('ob-theme', 'dark')
      document.documentElement.dataset.obTheme = 'dark'
    })
    await page.locator('.ob-update-pill').click()

    await expect(page.getByRole('heading', { name: 'App updates', exact: true })).toBeVisible()
    await expect(page.locator('.ob-update-card')).toBeVisible()

    // Confirm that the top UpdateBanner is hidden when on the updates section
    await expect(page.locator('.ob-update-banner')).toHaveCount(0)

    // Capture 04: dark mode Settings -> App updates confirming clean layout (no duplicate top banner)
    await page.screenshot({
      path: `${PREVIEW_DIR}/04-updates-settings-dark-clean.png`,
      fullPage: false
    })
  })
})
