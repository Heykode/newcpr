import assert from 'node:assert/strict'
import { mkdir, readFile } from 'node:fs/promises'
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
    plugins: [{
      name: 'isolated-account-catalog-test',
      configResolved(config) {
        config.server.proxy = {}
      },
    }],
  })
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-account-native-catalog-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
  const errors = []
  const unexpected = []
  const catalogQueries = []
  const tests = []
  const declaredModel = `declared-${'long-model-name-'.repeat(10)}`
  const catalog = { models: [{ slug: 'native-z', base_instructions: 'synthetic', future: { array: [true, null] } }, { slug: 'native-a' }] }
  const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/dev/api/**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())
    const path = url.pathname.replace(/^\/dev/, '')
    switch (path) {
      case '/api/admin/auth/status': return fulfill(route, { authenticated: true })
      case '/api/admin/system/version': return fulfill(route, { version: 'local-sample', buildType: 'test' })
      case '/api/admin/accounts':
        return fulfill(route, {
          items: accounts,
          page: { page: 1, pageSize: 20, total: accounts.length, totalPages: 1 },
          summary: { total: 3, normal: 3, disabled: 0, error: 0, rateLimited: 0, quotaExhausted: 0 },
        })
      case '/api/admin/account-groups':
        return fulfill(route, { items: [], page: { page: 1, pageSize: 200, total: 0, totalPages: 0 } })
      case '/api/admin/relogin': return fulfill(route, { items: reloginEntries, settings: { concurrency: 1, paused: false } })
      case '/api/admin/relogin/accounts/query': return fulfill(route, [])
      case '/api/admin/accounts/import-tasks': return fulfill(route, { items: [] })
      case '/api/admin/accounts/models':
        assert.equal(request.method(), 'GET')
        return fulfill(route, { models: [{ id: 'requested-model', label: 'requested-model' }] })
      case '/api/admin/accounts/models/catalog':
        assert.equal(request.method(), 'GET')
        catalogQueries.push(Object.fromEntries(url.searchParams))
        return fulfill(route, { catalog, modelCount: 2, observedAt: '2026-09-24T00:00:00Z' })
      case '/api/admin/accounts/connection-test': {
        assert.equal(request.method(), 'POST')
        assert.equal(url.search, '')
        tests.push(request.postDataJSON())
        const events = [
          { type: 'test_start', model: 'requested-model' },
          { type: 'content', text: 'Synthetic response' },
          { type: 'test_complete', success: true, upstreamResponseModel: tests.length === 1 ? declaredModel : null },
        ]
        return route.fulfill({
          contentType: 'text/event-stream',
          body: events.map(event => `data: ${JSON.stringify(event)}\n\n`).join(''),
        })
      }
      default:
        unexpected.push(`${request.method()} ${path}`)
        return route.fulfill({ status: 405, json: { code: 405, message: 'Synthetic fixture: unsupported operation', data: null } })
    }
  })
  try {
    await server.listen()
    const address = server.httpServer.address()
    assert.ok(address && typeof address !== 'string')
    await page.goto(`http://127.0.0.1:${address.port}/accounts`)
    const row = page.locator('tr[data-row-key="acct_sample_0"]')
    await row.waitFor()
    await row.getByRole('button', { name: '更多操作', exact: true }).click()
    const downloaded = page.waitForEvent('download')
    await page.getByRole('button', { name: '导出模型目录', exact: true }).click()
    const download = await downloaded
    assert.equal(download.suggestedFilename(), 'cpr-model-catalog-acct_sample_0.json')
    assert.deepEqual(JSON.parse(await readFile(await download.path(), 'utf8')), catalog)
    assert.deepEqual(catalogQueries, [{ accountId: 'acct_sample_0' }])
    assert.deepEqual(tests, [])

    await row.getByRole('button', { name: '更多操作', exact: true }).click()
    await page.getByRole('button', { name: '测试连接', exact: true }).click()
    const dialog = page.getByRole('dialog')
    await dialog.getByLabel('测试提示词', { exact: true }).fill('Synthetic diagnostic prompt\n第二行')
    await dialog.getByRole('button', { name: '开始测试', exact: true }).click()
    await dialog.getByText(declaredModel, { exact: true }).waitFor()
    assert.deepEqual(tests, [{
      accountId: 'acct_sample_0',
      modelId: 'requested-model',
      interface: 'responses',
      stream: true,
      prompt: 'Synthetic diagnostic prompt\n第二行',
    }])
    for (const theme of ['light', 'dark']) {
      await page.setViewportSize({ width: 1440, height: 1000 })
      if (await page.evaluate(() => document.documentElement.dataset.theme) !== theme) {
        await page.getByRole('button', { name: theme === 'dark' ? '切换暗黑模式' : '切换浅色模式', exact: true })
          .evaluate(button => button.click())
      }
      await page.waitForFunction(theme => document.documentElement.dataset.theme === theme, theme)
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 1000 })
        const bounds = await dialog.boundingBox()
        assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= width + 1)
        assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
        const returned = dialog.getByText(declaredModel, { exact: true })
        assert.ok(await returned.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
        await returned.scrollIntoViewIfNeeded()
        await page.screenshot({ path: `${output}/returned-model-${theme}-${width}.png`, fullPage: true })
      }
    }
    await dialog.getByRole('button', { name: '重新测试', exact: true }).click()
    await dialog.getByText('未返回', { exact: true }).waitFor()
    assert.equal(await dialog.getByText(declaredModel, { exact: true }).count(), 0)
    assert.equal(tests.length, 2)
    assert.equal(catalogQueries.length, 1)
    assert.deepEqual(unexpected, [])
    assert.deepEqual(errors, [])
    process.stdout.write('Passed: selected native catalog download, exact POST prompt, declared/absent model, light/dark 1440/390/320px; synthetic API only.\n')
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
