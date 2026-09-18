import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { fixtures, isolateNetwork } from './usage-columns.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const base = new URL(process.env.QA_BASE_URL || 'http://127.0.0.1:5198')
  assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname))
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-usage-state-qa'
  await mkdir(output, { recursive: true })
  const raw = `SYNTHETIC_STATE_${'a'.repeat(315)}_`
  assert.equal(raw.length, 332)
  const summary = {
    injected: true,
    preview: `${raw.slice(0, 8)}...${raw.slice(-6)}`,
    chars: raw.length,
    returnedChars: 332,
    returnedSame: true,
    transport: 'http_sse',
  }
  const template = fixtures().get('/api/admin/usage/records').items[0]
  const items = [
    summary,
    { ...summary, returnedChars: 356, returnedSame: false, transport: 'websocket' },
    { ...summary, returnedChars: null, returnedSame: null },
    { ...summary, injected: false, preview: null, chars: null, returnedChars: null, returnedSame: null },
    null,
  ].map((turnState, index) => ({ ...template, id: `request-state-${index}`, turnState }))
  const browser = await chromium.launch({
    headless: true,
    args: ['--disable-background-networking', '--no-proxy-server'],
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  })
  try {
    for (const theme of ['light', 'dark']) {
      for (const width of [1440, 390, 320]) {
        const context = await browser.newContext({
          viewport: { width, height: 900 },
          reducedMotion: 'reduce',
          colorScheme: theme,
          serviceWorkers: 'block',
        })
        const report = { blocked: [], api: [] }
        await isolateNetwork(context, base, report)
        const page = await context.newPage()
        const errors = []
        const details = []
        page.on('pageerror', error => errors.push(error.message))
        await page.route('**/dev/api/admin/usage/records?*', route => route.fulfill({
          json: { code: 200, message: 'ok', data: { items, currentPage: 1, pageSize: 10, total: items.length } },
        }))
        await page.route('**/dev/api/admin/usage/records/detail?*', (route) => {
          const id = new URL(route.request().url()).searchParams.get('id')
          details.push(id)
          return route.fulfill({ json: {
            code: 200,
            message: 'ok',
            data: { requestId: id, metadata: { turnState: { injectedState: raw } } },
          } })
        })
        try {
          await page.goto(new URL('/usage', base).href)
          const cells = page.locator('td[data-column-key="turnState"]')
          await cells.first().waitFor()
          assert.equal(await cells.count(), 5)
          assert.equal(details.length, 0, 'list rendering must not prefetch full State')
          assert.ok(!(await page.locator('body').textContent()).includes(raw))
          assert.match(await cells.nth(1).textContent(), /332 字符 · 回 356/)
          assert.match(await cells.nth(2).textContent(), /无回显/)
          assert.match(await cells.nth(3).textContent(), /—/)
          assert.match(await cells.nth(4).textContent(), /未记录/)
          await cells.nth(1).evaluate(element => element.scrollIntoView({ block: 'center', inline: 'center' }))
          await page.evaluate(() => document.fonts.ready)
          const compact = await cells.nth(1).evaluate((element) => {
            const cell = element.getBoundingClientRect()
            const count = element.querySelector('button span').getBoundingClientRect()
            const padding = Number.parseFloat(getComputedStyle(element).paddingRight)
            return { width: cell.width, countRight: count.right, contentRight: cell.right - padding }
          })
          assert.ok(Math.abs(compact.width - 144) <= 1, 'State column must not reserve extra space for its popover')
          assert.ok(compact.countRight <= compact.contentRight + 1, 'character counts must fit the compact column')
          await page.screenshot({ path: `${output}/${theme}-${width}-compact.png`, fullPage: true })
          const trigger = cells.nth(1).getByRole('button')
          if (width > 500)
            await trigger.hover()
          else
            await trigger.click()
          const panel = page.getByRole('dialog', { name: 'State 注入详情', exact: true })
          await panel.waitFor()
          await page.waitForFunction(value => document.querySelector('[role="dialog"] code')?.textContent === value, raw)
          assert.deepEqual(details, ['request-state-1'])
          assert.match(await panel.textContent(), /332 字符/)
          assert.match(await panel.textContent(), /WS 请求帧.*返回 356 字符.*值不同/s)
          await page.evaluate(() => document.fonts.ready)
          const bounds = await panel.evaluate(element => ({
            left: element.getBoundingClientRect().left,
            right: element.getBoundingClientRect().right,
            width: innerWidth,
            overflow: element.scrollWidth - element.clientWidth,
            codeOverflow: element.querySelector('code').scrollWidth - element.querySelector('code').clientWidth,
          }))
          assert.ok(bounds.left >= 0 && bounds.right <= bounds.width + 1)
          assert.ok(bounds.overflow <= 1 && bounds.codeOverflow <= 1)
          const panelRect = await panel.boundingBox()
          const triggerRect = await trigger.boundingBox()
          assert.ok(panelRect.y + panelRect.height <= triggerRect.y
            || triggerRect.y + triggerRect.height <= panelRect.y
            || panelRect.x + panelRect.width <= triggerRect.x
            || triggerRect.x + triggerRect.width <= panelRect.x, 'loaded details must not cover their trigger')
          await page.screenshot({ path: `${output}/${theme}-${width}.png`, fullPage: true })
          await trigger.dispatchEvent('mouseenter')
          await page.keyboard.press('Escape')
          await panel.waitFor({ state: 'hidden' })
          // A pending hover timer must not reopen sensitive details after Escape.
          await page.waitForTimeout(250)
          assert.equal(await panel.count(), 0)
          assert.ok(!(await page.locator('body').textContent()).includes(raw))
          const storage = await page.evaluate(() => `${JSON.stringify(localStorage)}${JSON.stringify(sessionStorage)}`)
          assert.ok(!storage.includes(raw))
          assert.deepEqual(errors, [])
          assert.deepEqual(report.blocked, [])
          process.stdout.write(`Passed State column, lazy full value and bounded popover: ${theme}-${width}\n`)
        }
        finally {
          await context.close()
        }
      }
    }
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
