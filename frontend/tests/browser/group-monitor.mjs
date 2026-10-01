import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'
import { groups, monitorResponse } from '../fixtures/monitor-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-group-monitor-qa'
  await mkdir(output, { recursive: true })
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    server: { host: '127.0.0.1', port: 0, proxy: {} },
    plugins: [{
      name: 'monitor-isolated-test',
      configResolved(config) { config.server.proxy = {} },
    }],
  })
  let browser
  try {
    await server.listen()
    const origin = server.resolvedUrls.local[0]
    browser = await chromium.launch({
      headless: true,
      ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
    })
    for (const width of [1440, 390, 320]) {
      const page = await browser.newPage({ viewport: { width, height: 900 }, reducedMotion: 'reduce' })
      const errors = []
      page.on('pageerror', error => errors.push(error.message))
      const start = Date.now()
      let sampledAt = start
      let mode = 'ready'
      let remaining = 500
      await page.clock.install({ time: new Date(start) })
      await page.route('**/dev/**', async (route) => {
        const url = new URL(route.request().url())
        assert.equal(url.pathname, '/dev/api/admin/account-groups/monitor')
        if (mode === 'failure')
          return route.fulfill({ status: 503, json: { code: 503, message: 'Synthetic failure', data: null } })
        const ids = (url.searchParams.get('groupIds') ?? '').split(',')
        const report = monitorResponse(ids)
        report.generatedAt = new Date(sampledAt).toISOString()
        report.items.forEach((item) => {
          item.remainingUsd = remaining
          item.expectedExpiryUsd = 50
          item.expiryStatus = 'ready'
          item.etaMinutes = 100
          item.etaStatus = 'ready'
          item.earliestResetAt = new Date(start - 1000).toISOString()
        })
        if (mode === 'pending') {
          report.pendingGroupIds = ids
          report.refreshing = true
          report.items = []
        }
        return route.fulfill({ json: { code: 200, message: 'ok', data: report } })
      })
      await page.goto(`${origin}tests/fixtures/group-monitor.html`)
      const card = page.getByRole('article', { name: groups[0].name, exact: true })
      await card.getByText('$500.00', { exact: true }).waitFor()
      await card.getByText('1h 40m', { exact: true }).waitFor()
      // A member reset already passed, but the current aggregate is still valid.
      assert.equal(await card.getByText('未知', { exact: true }).count(), 0)
      const advance = async (milliseconds) => {
        const response = page.waitForResponse(result => result.url().includes('/account-groups/monitor'))
        await page.clock.fastForward(milliseconds)
        await response
      }
      remaining = 600
      sampledAt = start + 10_000
      await advance(10_000)
      await card.getByText('$600.00', { exact: true }).waitFor()
      mode = 'failure'
      await advance(10_000)
      await page.getByText('监控更新失败', { exact: true }).waitFor()
      await card.getByText('$600.00', { exact: true }).waitFor()
      mode = 'pending'
      await advance(10_000)
      await card.getByText('$600.00', { exact: true }).waitFor()
      await advance(40_000)
      await card.getByText('未知', { exact: true }).first().waitFor()
      assert.equal(await card.getByText('$600.00', { exact: true }).count(), 0)
      mode = 'ready'
      sampledAt = start + 80_000
      await advance(10_000)
      await card.getByText('$600.00', { exact: true }).waitFor()
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
      await page.screenshot({ path: `${output}/monitor-${width}.png`, fullPage: true })
      assert.deepEqual(errors, [])
      await page.close()
    }
    process.stdout.write('Monitor reset, poll, failure, pending, expiry and recovery passed at 1440/390/320px.\n')
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
