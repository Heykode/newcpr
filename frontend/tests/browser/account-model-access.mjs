import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { accounts as fixtures } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  })
  const base = process.env.QA_BASE_URL || 'http://127.0.0.1:5245'
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-model-access-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
  const accounts = fixtures.map(account => ({
    ...structuredClone(account),
    turnStateInjectionEnabled: false,
    modelAccess: { mode: 'all', models: [] },
  }))
  const updates = []
  const errors = []
  let catalogFails = false
  let catalogRequests = 0
  page.on('pageerror', error => errors.push(error.message))
  page.on('response', (response) => {
    if (new URL(response.url()).pathname.startsWith('/assets/') && response.status() >= 400)
      errors.push(`Static asset failed: ${response.status()}`)
  })
  // All API traffic stays inside this synthetic browser fixture.
  await page.route(/\/(?:dev\/)?api\//, async (route) => {
    const request = route.request()
    const path = new URL(request.url()).pathname.replace(/^\/dev(?=\/)/, '')
    let data
    let status = 200
    if (path === '/api/admin/auth/status') {
      data = { authenticated: true }
    }
    else if (path === '/api/admin/system/version') {
      data = { version: 'local-test', buildType: 'test' }
    }
    else if (path === '/api/admin/accounts') {
      data = { items: accounts, page: { page: 1, pageSize: 20, total: 3, totalPages: 1 }, summary: { total: 3, normal: 3, error: 0, rateLimited: 0, disabled: 0, quotaExhausted: 0 } }
    }
    else if (path === '/api/admin/accounts/models' || path === '/api/admin/accounts/models/refresh') {
      catalogRequests += 1
      status = catalogFails ? 503 : 200
      data = { models: ['model-a', 'model-b', `long-model-${'x'.repeat(100)}`].map(id => ({ id, label: id })) }
    }
    else if (path === '/api/admin/accounts/update' || path === '/api/admin/accounts/batch-update') {
      const body = request.postDataJSON()
      updates.push(body)
      for (const account of accounts.filter(item => (body.accountIds || [body.accountId]).includes(item.id))) {
        if (body.modelAccess)
          account.modelAccess = structuredClone(body.modelAccess)
      }
      data = { accountId: body.accountId, accountIds: body.accountIds, configRevision: updates.length + 1 }
    }
    else if (path === '/api/admin/relogin/accounts/query') {
      data = []
    }
    else if (path === '/api/admin/ipv6-egress') {
      data = { revision: 1, defaultMode: 'unchanged', addresses: [], accountOverrides: {}, fixedBindings: {} }
    }
    else {
      data = { items: [], page: { page: 1, pageSize: 200, total: 0, totalPages: 0 } }
    }
    await route.fulfill({ status, contentType: 'application/json', body: JSON.stringify({ code: status, message: status === 200 ? 'ok' : 'synthetic catalog unavailable', data }) })
  })
  const dialog = page.getByRole('dialog')
  const row = id => page.locator(`tbody tr[data-row-key="${id}"]`)
  const first = () => row('acct_sample_0')
  async function close() {
    await dialog.getByRole('button', { name: '取消未保存更改', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
  }
  async function open() {
    await first().getByRole('button', { name: '编辑账号', exact: true }).click()
    await dialog.getByRole('radio', { name: '不限制', exact: true }).waitFor()
  }
  async function layouts(label) {
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: width < 500 ? 844 : 1000 })
      assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
      await page.evaluate(() => document.fonts.ready)
      await page.screenshot({ path: `${output}/${label}-${width}.png`, fullPage: true })
    }
    await page.setViewportSize({ width: 1440, height: 1000 })
  }
  try {
    await page.goto(`${base}/accounts`)
    await first().waitFor()
    await open()
    assert.equal(catalogRequests, 0, 'unrestricted editing needs no catalog request')
    await dialog.getByRole('radio', { name: '白名单', exact: true }).click()
    await dialog.getByRole('checkbox', { name: 'model-a', exact: true }).locator('..').click()
    await layouts('edit-allowlist')
    await dialog.getByRole('button', { name: '保存账号设置', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.deepEqual(updates[0].modelAccess, { mode: 'allowlist', models: ['model-a'] })
    await open()
    assert.equal(await dialog.getByRole('checkbox', { name: 'model-a', exact: true }).isChecked(), true)
    await close()
    catalogFails = true
    await open()
    await dialog.getByRole('alert').filter({ hasText: '模型列表加载失败' }).waitFor()
    assert.equal(await dialog.getByRole('checkbox', { name: 'model-a', exact: true }).isChecked(), true)
    const input = dialog.getByRole('textbox', { name: '搜索或添加模型', exact: true })
    await input.fill('model-*')
    await input.press('Enter')
    await dialog.getByRole('alert').filter({ hasText: '不支持通配符' }).waitFor()
    await input.fill('model-manual')
    await input.press('Enter')
    await dialog.getByRole('checkbox', { name: 'model-manual', exact: true }).waitFor()
    await layouts('catalog-error')
    await dialog.getByRole('button', { name: '保存账号设置', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.deepEqual(updates[1].modelAccess.models, ['model-a', 'model-manual'])
    catalogFails = false
    for (const id of ['acct_sample_0', 'acct_sample_1'])
      await row(id).getByRole('checkbox', { name: '选择账号', exact: true }).locator('..').click()
    await page.getByRole('button', { name: '批量编辑账号', exact: true }).click()
    assert.equal(await dialog.getByRole('radio', { name: '白名单', exact: true }).isDisabled(), true)
    await dialog.getByRole('checkbox', { name: '应用模型限制更改', exact: true }).locator('..').click()
    await dialog.getByRole('radio', { name: '黑名单', exact: true }).click()
    await dialog.getByRole('checkbox', { name: 'model-b', exact: true }).locator('..').click()
    await layouts('batch-denylist')
    await dialog.getByRole('button', { name: '保存更改', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.deepEqual(updates[2], { accountIds: ['acct_sample_0', 'acct_sample_1'], modelAccess: { mode: 'denylist', models: ['model-b'] } })
    await page.getByRole('button', { name: '导入账号', exact: true }).click()
    assert.equal(await dialog.getByRole('radio', { name: '保留', exact: true }).isChecked(), true)
    await dialog.getByRole('radio', { name: '白名单', exact: true }).click()
    assert.equal(await dialog.getByRole('button', { name: '继续导入', exact: true }).isDisabled(), true)
    await dialog.getByRole('textbox', { name: '搜索或添加模型', exact: true }).fill('model-a')
    await dialog.getByRole('textbox', { name: '搜索或添加模型', exact: true }).press('Enter')
    await dialog.getByRole('button', { name: 'OpenAI', exact: true }).click()
    assert.equal(await dialog.getByRole('button', { name: '继续导入', exact: true }).isEnabled(), true)
    await layouts('import')
    assert.deepEqual(errors, [])
    process.stdout.write(`${JSON.stringify({ result: 'passed', updates: updates.length, catalogRequests, output })}\n`)
  }
  catch (error) {
    await page.screenshot({ path: `${output}/failure.png`, fullPage: true })
    throw error
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
