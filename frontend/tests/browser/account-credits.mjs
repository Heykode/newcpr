import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'
import { accounts } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    server: { host: '127.0.0.1', port: 0 },
    plugins: [{ name: 'isolated-credits-test', configResolved(config) { config.server.proxy = {} } }],
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-account-credits-qa'
  let browser
  try {
    await server.listen()
    const base = `http://127.0.0.1:${server.httpServer.address().port}`
    browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH || undefined })
    await mkdir(output, { recursive: true })
    for (const width of [1440, 390, 320]) {
      const page = await browser.newPage({ viewport: { width, height: 1000 }, reducedMotion: 'reduce' })
      const errors = []
      const unexpected = []
      let resetReads = 0
      let autoPolicy = { accountId: accounts[0].id, revision: 0, config: { enabled: false, fiveHourUsedMillis: 100000, sevenDayUsedMillis: 100000 }, checkedAt: null, message: null }
      let autoWrites = 0
      let failSave = false
      const row = structuredClone(accounts[0])
      row.quota.credits = { hasCredits: true, unlimited: false, balance: '123.45' }
      page.on('pageerror', error => errors.push(error.message))
      const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
      await page.route('**/*', (route) => {
        const request = route.request()
        const url = new URL(request.url())
        if (url.origin !== base) {
          unexpected.push('external request')
          return route.abort()
        }
        if (!url.pathname.startsWith('/dev/api/'))
          return route.continue()
        const path = url.pathname.replace(/^\/dev/, '')
        if (path === '/api/admin/accounts/reset-credits/automatic') {
          if (request.method() === 'POST') {
            autoWrites++
            if (failSave)
              return route.fulfill({ status: 503, json: { code: 503, message: 'fixture unavailable' } })
            const body = request.postDataJSON()
            assert.equal(body.accountId, row.id)
            assert.equal(body.revision, autoPolicy.revision)
            autoPolicy = { ...autoPolicy, config: body.config, revision: autoPolicy.revision + 1 }
          }
          return fulfill(route, autoPolicy)
        }
        if (path === '/api/admin/auth/refresh' && request.method() === 'POST')
          return fulfill(route, { authenticated: true })
        if (['/api/admin/relogin/accounts/query', '/api/admin/accounts/reset-credits/cache'].includes(path) && request.method() === 'POST')
          return fulfill(route, [])
        if (request.method() !== 'GET') {
          unexpected.push(`${request.method()} ${path}`)
          return route.abort()
        }
        switch (path) {
          case '/api/admin/auth/status': return fulfill(route, { authenticated: true })
          case '/api/admin/system/version': return fulfill(route, { version: 'local-fixture', buildType: 'test' })
          case '/api/admin/accounts': return fulfill(route, {
            items: [row],
            page: { page: 1, pageSize: 20, total: 1, totalPages: 1 },
            summary: { total: 1, normal: 1, disabled: 0, error: 0, rateLimited: 0, quotaExhausted: 0 },
          })
          case '/api/admin/account-groups': return fulfill(route, { items: [], page: { page: 1, pageSize: 200, total: 0, totalPages: 0 } })
          case '/api/admin/account-templates':
          case '/api/admin/accounts/import-tasks': return fulfill(route, { items: [] })
          case '/api/admin/accounts/reset-credits/cache': return fulfill(route, [])
          case '/api/admin/accounts/reset-credits/batches': return fulfill(route, [])
          case '/api/admin/relogin': return fulfill(route, { items: [], settings: { concurrency: 1, paused: false } })
          case '/api/admin/accounts/reset-credits':
            resetReads++
            return fulfill(route, { availableCount: 0, credits: [], pending: null })
          default:
            unexpected.push(path)
            return route.abort()
        }
      })
      try {
        await page.goto(`${base}/accounts`)
        const tableRow = page.locator(`tr[data-row-key="${row.id}"]`)
        const listBalance = tableRow.getByLabel('Codex 点数', { exact: true }).getByText('123.45', { exact: true })
        await listBalance.waitFor()
        if (width >= 1000) {
          assert.ok(await listBalance.evaluate((el) => {
            const bounds = el.getBoundingClientRect()
            const hit = document.elementFromPoint(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2)
            return hit === el || el.contains(hit)
          }), 'sticky actions must not cover the compact balance')
        }
        assert.equal(resetReads, 0, 'showing credits must not require reset-card lookup')
        await tableRow.locator('button[title="展开统计"]').click()
        const panel = page.locator('section').filter({ has: page.getByRole('heading', { name: '账号额度', exact: true }) }).last()
        const credits = panel.getByLabel('Codex 点数', { exact: true })
        await credits.getByText('123.45', { exact: true }).waitFor()
        await credits.scrollIntoViewIfNeeded()
        assert.equal(await credits.evaluate(el => el.scrollWidth <= el.clientWidth + 1), true)
        const bounds = await credits.getByText('123.45', { exact: true }).boundingBox()
        assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= width, 'balance must be inside the viewport, not merely in the DOM')
        await page.screenshot({ path: `${output}/credits-${width}.png` })
        await panel.getByRole('button', { name: '查看主动重置卡', exact: true }).click()
        await page.getByRole('dialog').getByText('可用 0 次', { exact: true }).waitFor()
        const auto = page.getByRole('dialog').getByRole('region', { name: '自动使用重置卡' })
        const toggle = auto.getByRole('switch', { name: '自动重置', exact: true })
        await toggle.waitFor()
        assert.equal(await toggle.isChecked(), false)
        assert.equal(autoWrites, 0, 'opening controls must not authorize consumption')
        await toggle.locator('..').click()
        assert.equal(await toggle.isChecked(), true)
        await auto.getByRole('spinbutton', { name: '5 小时已用阈值', exact: true }).fill('75.5')
        await auto.getByRole('spinbutton', { name: '7 天已用阈值', exact: true }).fill('0')
        await auto.getByRole('button', { name: '保存自动重置', exact: true }).click()
        await auto.getByText('设置已保存；已发送的重置请求不受关闭操作影响。').waitFor()
        assert.deepEqual(autoPolicy.config, { enabled: true, fiveHourUsedMillis: 75500, sevenDayUsedMillis: 0 })
        assert.ok(await auto.evaluate(el => el.scrollWidth <= el.clientWidth + 1))
        await auto.scrollIntoViewIfNeeded()
        await page.screenshot({ path: `${output}/auto-reset-${width}.png`, animations: 'disabled' })
        failSave = true
        await toggle.locator('..').click()
        await auto.getByRole('button', { name: '保存自动重置', exact: true }).click()
        await auto.getByRole('alert').waitFor()
        assert.equal(await auto.getByRole('button', { name: '保存自动重置', exact: true }).isDisabled(), true)
        failSave = false
        await auto.getByRole('button', { name: '刷新自动重置设置', exact: true }).click()
        await auto.getByRole('alert').waitFor({ state: 'hidden' })
        await auto.locator('input[role="switch"]:checked').waitFor({ state: 'attached' })
        assert.equal(await toggle.isChecked(), true, 'fresh authoritative policy wins after uncertain save')
        assert.equal(resetReads, 1)
        assert.equal(await credits.getByText('123.45', { exact: true }).count(), 1)

        for (const [value, expected] of [
          [{ hasCredits: false, unlimited: false, balance: '0' }, '0'],
          [{ hasCredits: false, unlimited: false, balance: null }, '暂无可用点数'],
          [{ hasCredits: true, unlimited: false, balance: null }, '未提供余额'],
          [{ hasCredits: true, unlimited: true, balance: null }, '无限'],
          [{ hasCredits: true, unlimited: false, balance: '12345678901234567890.125' }, '12,345,678,901,234,567,890.125'],
          [null, '未提供余额'],
        ]) {
          row.quota.credits = value
          await page.reload()
          await tableRow.getByLabel('Codex 点数', { exact: true }).getByText(expected, { exact: true }).waitFor()
          await tableRow.locator('button[title="展开统计"]').click()
          await credits.getByText(expected, { exact: true }).waitFor()
          assert.equal(await credits.evaluate(el => el.scrollWidth <= el.clientWidth + 1), true)
          const valueBounds = await credits.getByText(expected, { exact: true }).boundingBox()
          assert.ok(valueBounds.x >= 0 && valueBounds.x + valueBounds.width <= width)
        }
        row.provider = 'xai'
        await page.reload()
        await tableRow.waitFor()
        assert.equal(await tableRow.getByLabel('Codex 点数', { exact: true }).count(), 0)
        await tableRow.locator('button[title="展开统计"]').click()
        await panel.waitFor()
        assert.equal(await panel.getByLabel('Codex 点数', { exact: true }).count(), 0)
        assert.deepEqual(errors, [])
        assert.deepEqual(unexpected, [])
        process.stdout.write(`PASS ${width}px: list/panel, no reset card, zero/unknown/unlimited/precision, provider isolation; no account writes.\n`)
      }
      catch (error) {
        await page.screenshot({ path: `${output}/failure-${width}.png`, fullPage: true })
        console.error({ errors, unexpected })
        throw error
      }
      finally { await page.close() }
    }
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
