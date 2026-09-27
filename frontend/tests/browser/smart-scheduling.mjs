import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'

async function main() {
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    server: { host: '127.0.0.1', port: 0 },
    plugins: [{ name: 'isolated-smart-qa', configResolved(config) { config.server.proxy = {} } }],
  })
  await server.listen()
  const origin = `http://127.0.0.1:${server.httpServer.address().port}`
  if (process.argv.includes('--preview')) {
    process.stdout.write(`${origin}/tests/fixtures/smart-scheduling.html\n`)
    return
  }
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-smart-scheduling-qa'
  await mkdir(output, { recursive: true })
  try {
    for (const width of [1440, 390, 320]) {
      const page = await browser.newPage({ viewport: { width, height: 1050 }, reducedMotion: 'reduce' })
      const errors = []
      page.on('pageerror', error => errors.push(error.message))
      await page.route('**/*', (route) => {
        const url = new URL(route.request().url())
        return url.origin === origin && !/^\/(?:dev|api)(?:\/|$)/.test(url.pathname) ? route.continue() : route.abort()
      })
      await page.goto(`${origin}/tests/fixtures/smart-scheduling.html`)
      const load = page.getByRole('spinbutton', { name: '负载权重', exact: true })
      try {
        await load.waitFor()
      }
      catch (error) {
        await page.screenshot({ path: `${output}/failure-${width}.png`, fullPage: true })
        throw new Error(`${error.message}\n${errors.join('\n')}\n${await page.locator('body').textContent()}`)
      }
      assert.equal(await load.inputValue(), '1')
      assert.equal(await page.getByRole('spinbutton', { name: '剩余额度权重' }).inputValue(), '0.8')
      const switchback = page.getByRole('switch', { name: '主动回切高权重账号' })
      assert.equal(await switchback.isChecked(), false)
      await load.fill('2.5')
      await switchback.setChecked(true, { force: true })
      await page.getByRole('button', { name: '恢复默认' }).click()
      assert.equal(await load.inputValue(), '1')
      assert.equal(await switchback.isChecked(), false)
      await page.screenshot({ path: `${output}/smart-${width}.png`, fullPage: true })
      assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth))
      for (const box of await page.getByRole('spinbutton').all()) {
        const bounds = await box.boundingBox()
        assert(bounds.x >= 0 && bounds.x + bounds.width <= width)
      }
      assert.deepEqual(errors, [])
      await page.close()
    }
    process.stdout.write('Smart controls passed at 1440, 390 and 320 pixels with all external/API traffic blocked.\n')
  }
  finally {
    await browser.close()
    await server.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
