import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const browser = await chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) })
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    server: { host: '127.0.0.1', port: 0 },
    plugins: [{ name: 'isolated-theme-preview', configResolved(config) { config.server.proxy = {} } }],
  })
  await server.listen()
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-theme-preview-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ locale: 'zh-CN', viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
  const errors = []
  page.on('pageerror', (error) => {
    errors.push(error.message)
  })
  page.on('console', (message) => {
    if (message.type() === 'error')
      console.error(message.text())
  })
  await page.route(/\/(?:dev\/)?api\//, (route) => {
    const path = new URL(route.request().url()).pathname.replace(/^\/dev(?=\/)/, '')
    if (!path.startsWith('/api/'))
      return route.continue()
    return route.fulfill({ contentType: 'application/json', body: JSON.stringify({ code: 200, message: 'ok', data: { authenticated: true, items: [], version: 'test' } }) })
  })
  let releasePreview
  const previewGate = new Promise((resolve) => {
    releasePreview = resolve
  })
  let requestedPreview
  const requested = new Promise((resolve) => {
    requestedPreview = resolve
  })
  await page.route(/\/ThemeDashboardPreview\.vue(?:\?|$)/, async (route) => {
    requestedPreview()
    await previewGate
    await route.continue()
  })
  let requestTimeout
  try {
    await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/theme`, { waitUntil: 'domcontentloaded' })
    await Promise.race([requested, new Promise((_, reject) => {
      requestTimeout = setTimeout(() => reject(new Error(`No preview request at ${page.url()}`)), 20000)
    })])
    clearTimeout(requestTimeout)
    await page.locator('[data-theme-preview-stage]').waitFor({ state: 'visible' })
    assert.equal((await page.locator('[data-theme-preview-board]').textContent()).trim(), '', 'canvas appears before preview module completes')
    releasePreview()
    await page.waitForFunction(() => document.querySelector('[data-theme-preview-shadow-host]')?.shadowRoot?.querySelector('[data-theme-preview-board]')?.textContent.trim().length > 20)
    await page.getByRole('radio', { name: '组件概览', exact: true }).click()
    await page.locator('[data-theme-preview-board] h1').waitFor()
    const styleCount = await page.locator('[data-theme-preview-shadow-host]').evaluate(host => host.shadowRoot.querySelector('style[data-theme-preview-styles]').textContent.length)
    assert.ok(styleCount > 1000, 'lazy preview keeps application styles in the shadow tree')
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 1000 })
      if (width < 768)
        await page.getByRole('button', { name: '预览', exact: true }).click()
      await page.evaluate(() => document.fonts.ready)
      await page.screenshot({ path: `${output}/theme-${width}.png`, fullPage: true })
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1))
    }
    assert.deepEqual(errors, [])
    process.stdout.write(`${JSON.stringify({ result: 'passed', output })}\n`)
  }
  finally {
    clearTimeout(requestTimeout)
    releasePreview()
    await browser.close()
    await server.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
