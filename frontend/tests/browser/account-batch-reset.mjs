import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'
import { accounts, reloginEntries } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    server: { host: '127.0.0.1', port: 0 },
    plugins: [{ name: 'isolated-reset-test', configResolved(config) { config.server.proxy = {} } }],
  })
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-batch-reset-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
  const errors = []
  const unexpected = []
  let consumes = 0
  let previews = 0
  let batches = []
  let inventory = []
  const fixtureAccounts = accounts.slice(0, 3).map(account => ({ ...account, quota: { ...account.quota, credits: null } }))
  const card = { id: 'card-soon', status: 'available', title: '额度重置', resetType: 'codex', expiresAt: '2026-10-02T01:00:00Z' }
  let currentPreview
  const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/*', async (route) => {
    const url = new URL(route.request().url())
    if (url.hostname !== '127.0.0.1' && url.hostname !== 'localhost') {
      unexpected.push('external request')
      return route.abort()
    }
    if (!url.pathname.startsWith('/dev/api/'))
      return route.continue()
    const path = url.pathname.replace(/^\/dev/, '')
    switch (path) {
      case '/api/admin/auth/status': return fulfill(route, { authenticated: true })
      case '/api/admin/auth/refresh': return fulfill(route, { authenticated: true })
      case '/api/admin/system/version': return fulfill(route, { version: 'local-fixture', buildType: 'test' })
      case '/api/admin/accounts':
        return fulfill(route, {
          items: fixtureAccounts,
          page: { page: 1, pageSize: 20, total: fixtureAccounts.length, totalPages: 1 },
          summary: { total: 3, normal: 3, disabled: 0, error: 0, rateLimited: 0, quotaExhausted: 0 },
        })
      case '/api/admin/account-groups':
        return fulfill(route, { items: [], page: { page: 1, pageSize: 200, total: 0, totalPages: 0 } })
      case '/api/admin/account-templates': return fulfill(route, { items: [] })
      case '/api/admin/relogin': return fulfill(route, { items: reloginEntries, settings: { concurrency: 1, paused: false } })
      case '/api/admin/relogin/accounts/query': return fulfill(route, [])
      case '/api/admin/accounts/import-tasks': return fulfill(route, { items: [] })
      case '/api/admin/accounts/reset-credits/cache': return fulfill(route, inventory)
      case '/api/admin/accounts/reset-credits/batches': return fulfill(route, batches)
      case '/api/admin/accounts/reset-credits/refresh':
        inventory = fixtureAccounts.map((a, index) => ({
          accountId: a.id,
          checkedAt: '2026-10-01T00:00:00Z',
          pending: null,
          credits: index === 2 ? null : { availableCount: index === 0 ? 2 : 0, credits: index === 0 ? [card] : [] },
          error: index === 2 ? '查询失败' : null,
        }))
        return fulfill(route, inventory)
      case '/api/admin/accounts/reset-credits/preview':
        previews++
        currentPreview = {
          id: `batch-${previews}`,
          confirmed: false,
          resetType: null,
          createdAt: '2026-10-01T00:00:00Z',
          items: fixtureAccounts.map((a, index) => ({
            accountId: a.id,
            availableCount: index === 2 ? null : index === 0 ? 2 : 0,
            credit: index === 0 ? card : null,
            redeemRequestId: `operation-${index}`,
            status: index === 0 ? 'ready' : 'skipped',
            message: ['等待确认', '没有可用重置次数', '查询失败'][index],
          })),
        }
        return fulfill(route, currentPreview)
      case '/api/admin/accounts/reset-credits/confirm': {
        assert.equal(route.request().postDataJSON().id, currentPreview.id)
        consumes++
        const batch = { ...currentPreview, confirmed: true, items: currentPreview.items.map(i => ({ ...i, status: i.status === 'ready' ? 'succeeded' : i.status, message: i.status === 'ready' ? '额度已重置' : i.message })) }
        batches = [batch]
        return fulfill(route, batch)
      }
      default:
        unexpected.push(path)
        return route.fulfill({ status: 405, json: { code: 405, message: 'Unsupported fixture request' } })
    }
  })
  try {
    await server.listen()
    const address = server.httpServer.address()
    await page.goto(`http://127.0.0.1:${address.port}/accounts`)
    await page.locator('tr[data-row-key]').first().waitFor()
    await page.getByRole('checkbox', { name: '选择当前页账号', exact: true }).locator('..').click()
    await page.getByRole('button', { name: '刷新重置次数', exact: true }).click()
    await page.locator('td[data-column-key="resetCredits"]').filter({ hasText: '2 次' }).waitFor()
    assert.equal(consumes, 0)
    await page.getByRole('button', { name: '使用重置次数', exact: true }).click()
    const dialog = page.getByRole('dialog')
    await dialog.getByRole('button', { name: '确认使用 1 次', exact: true }).waitFor()
    assert.equal(consumes, 0)
    await dialog.getByRole('button', { name: '取消', exact: true }).click()
    assert.equal(consumes, 0)
    await page.getByRole('button', { name: '使用重置次数', exact: true }).click()
    await dialog.getByRole('button', { name: '确认使用 1 次', exact: true }).waitFor()
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 })
      const bounds = await dialog.boundingBox()
      assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= width + 1)
      assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
      await page.screenshot({ path: `${output}/preview-${width}.png`, fullPage: true })
    }
    await dialog.getByRole('button', { name: '确认使用 1 次', exact: true }).click()
    await dialog.getByText('额度已重置', { exact: true }).waitFor()
    assert.equal(consumes, 1)
    await dialog.getByRole('button', { name: '关闭', exact: true }).last().click()
    await page.reload()
    await page.getByRole('button', { name: '重置记录', exact: true }).click()
    await dialog.getByText('额度已重置', { exact: true }).waitFor()
    assert.equal(consumes, 1, 'reload/history must not submit another consumption')
    assert.deepEqual(errors, [])
    assert.deepEqual(unexpected, [])
    process.stdout.write('Passed: batch query, skip empty/error, cancel, confirm once, durable history, 1440/390/320px; synthetic API only.\n')
  }
  catch (error) {
    await page.screenshot({ path: `${output}/failure.png`, fullPage: true })
    throw error
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
