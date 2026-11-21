import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'
import { fixtures, isolateNetwork } from './usage-columns.mjs'

async function main() {
  const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href)
  const output = process.env.QA_OUTPUT_DIR || '/tmp/newcpr-v3540-browser/usage'
  await mkdir(output, { recursive: true })
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    server: { host: '127.0.0.1', port: 43155, strictPort: true, hmr: false },
    plugins: [{
      name: 'isolated-usage',
      configResolved(config) {
        config.server.proxy = {}
      },
    }],
  })
  await server.listen()
  const base = new URL(`http://127.0.0.1:${server.httpServer.address().port}`)
  const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH })
  try {
    for (const width of [1440, 390, 320]) {
      const context = await browser.newContext({ locale: 'zh-CN', viewport: { width, height: 1000 }, reducedMotion: 'reduce', serviceWorkers: 'block' })
      const report = { blocked: [], api: [], errors: [] }
      await isolateNetwork(context, base, report)
      const fixture = fixtures().get('/api/admin/usage/records').items[0]
      const message = `Synthetic upstream error: ${'a long diagnostic detail '.repeat(40)}`
      await context.route('**/*', async (route) => {
        const path = new URL(route.request().url()).pathname.replace(/^\/dev(?=\/api\/)/u, '')
        if (path !== '/api/admin/usage/records')
          return route.fallback()
        return route.fulfill({ json: { code: 200, message: 'ok', data: { currentPage: 1, pageSize: 10, total: 1, items: [{ ...fixture, outcome: 'incomplete', errorKind: 'upstream_unavailable', errorCode: 'websocket_close_1006', errorMessage: message, websocketCloseCode: 1006, clientStatusCode: 502, upstreamStatusCode: 503 }] } } })
      })
      const page = await context.newPage()
      page.on('pageerror', error => report.errors.push(error.message))
      await page.goto(new URL('/usage', base).href)
      await page.getByRole('radiogroup', { name: '请求状态', exact: true }).getByRole('radio', { name: '错误', exact: true }).click()
      const table = page.locator('table:visible')
      const errorCell = table.locator('tbody [data-column-key="errorMessage"]').first()
      await errorCell.waitFor()
      const keys = await table.locator('thead [data-column-key]').evaluateAll(nodes => nodes.map(node => node.dataset.columnKey))
      assert.deepEqual(keys.slice(0, 2), ['errorMessage', 'createdAtDisplay'])
      assert.equal(keys.at(-1), 'actions')
      const action = table.locator('tbody [data-column-key="actions"]').first()
      assert.equal(await errorCell.evaluate(node => getComputedStyle(node).position), 'sticky')
      assert.equal(await action.evaluate(node => getComputedStyle(node).position), 'sticky')
      const trigger = errorCell.getByRole('button')
      await trigger.click()
      const popup = page.getByRole('tooltip').filter({ hasText: message })
      await popup.waitFor()
      const box = await popup.boundingBox()
      assert.ok(box.x >= -1 && box.x + box.width <= width + 1, 'error detail remains inside viewport')
      assert.ok(box.height <= 650, 'long detail is scrollable and bounded')
      assert.ok((await popup.textContent()).includes('1006'))
      await page.screenshot({ path: `${output}/error-${width}.png` })
      await page.keyboard.press('Escape')
      await popup.waitFor({ state: 'hidden' })
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
      assert.deepEqual(report.blocked, [])
      assert.deepEqual(report.errors, [])
      await context.close()
    }
    process.stdout.write('PASS: error summary/detail, close-code, sticky columns/order and bounded mobile popover at 1440/390/320px\n')
  }
  finally {
    await browser.close()
    await server.close()
  }
}
main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
