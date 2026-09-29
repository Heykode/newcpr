import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'

async function main() {
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    server: { host: '127.0.0.1', port: 0 },
    plugins: [{ name: 'isolated-prefetch-qa', configResolved(config) { config.server.proxy = {} } }],
  })
  await server.listen()
  const origin = `http://127.0.0.1:${server.httpServer.address().port}`
  let browser
  try {
    const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
    browser = await chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) })
    const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-stream-prefetch-qa'
    await mkdir(output, { recursive: true })
    for (const width of [1440, 390, 320]) {
      const page = await browser.newPage({ viewport: { width, height: 1050 }, reducedMotion: 'reduce' })
      const errors = []
      page.on('pageerror', error => errors.push(error.message))
      await page.route('**/*', (route) => {
        const url = new URL(route.request().url())
        return url.origin === origin && !/^\/(?:dev|api)(?:\/|$)/.test(url.pathname) ? route.continue() : route.abort()
      })
      await page.goto(`${origin}/tests/fixtures/stream-prefetch.html`)
      await page.getByRole('button', { name: '共享重试与原生连接高级参数' }).click()
      const input = page.getByRole('spinbutton', { name: '提交前缓冲阈值（KiB）', exact: true })
      assert.equal(await input.inputValue(), '128')
      assert.equal(await input.getAttribute('max'), null)
      for (const [kib, bytes] of [['0', '0'], ['256', '262144'], ['0.5', '512'], ['131072', '134217728']]) {
        await input.fill(kib)
        await input.blur()
        assert.equal(await page.getByTestId('saved-bytes').textContent(), bytes)
      }
      await input.fill('128')
      await page.screenshot({ path: `${output}/prefetch-${width}.png`, fullPage: true })
      assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth))
      const bounds = await input.boundingBox()
      assert(bounds.x >= 0 && bounds.x + bounds.width <= width)
      assert.deepEqual(errors, [])
      await page.close()
    }
    process.stdout.write('Prefetch control passed at 1440, 390 and 320 pixels; all external/API traffic blocked.\n')
  }
  finally {
    await browser?.close()
    await server.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
