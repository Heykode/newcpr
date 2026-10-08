import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { accounts as sampleAccounts } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const port = process.env.QA_PORT || '5199'
  const base = `http://127.0.0.1:${port}`
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-template-model-access'
  await mkdir(output, { recursive: true })
  const server = spawn(process.execPath, ['tests/relogin-count-preview.mjs'], {
    env: { ...process.env, QA_PORT: port },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  let serverOutput = ''
  let serverError = ''
  server.stdout.on('data', (chunk) => {
    serverOutput += chunk
  })
  server.stderr.on('data', (chunk) => {
    serverError += chunk
  })
  let browser
  try {
    let ready = false
    for (let i = 0; i < 100; i++) {
      assert.equal(server.exitCode, null, `fixture server exited: ${serverError}`)
      try {
        if (serverOutput.includes('Local sample only:') && (await fetch(`${base}/accounts`)).ok) {
          ready = true
          break
        }
      }
      catch {}
      await new Promise(resolve => setTimeout(resolve, 100))
    }
    assert.ok(ready, 'isolated fixture server must start')
    browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH || undefined })
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
    const errors = []
    const saves = []
    const applications = []
    let catalogRequests = 0
    let failKnownCatalog = true
    const knownCatalogPages = []
    const accounts = structuredClone(sampleAccounts)
    accounts[0].modelAccess = { mode: 'denylist', models: ['model-original'] }
    let template = {
      id: 'template-policy',
      revision: 1,
      config: { name: '模型限制模板', enabled: true, concurrencyLimit: null, weight: 1, groupIds: [], preserveOutboundProxy: true, outboundProxyId: null },
    }
    const respond = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
    page.on('pageerror', error => errors.push(error.message))
    page.on('response', (response) => {
      const pathname = new URL(response.url()).pathname
      if (pathname.startsWith('/dev/api/') && response.status() >= 400 && pathname !== '/dev/api/admin/quality-ops/models' && !pathname.startsWith('/dev/api/admin/accounts/reset-credits/'))
        errors.push(`${response.status()} ${pathname}`)
    })
    await page.route('**/dev/api/**', async (route) => {
      const request = route.request()
      const pathname = new URL(request.url()).pathname.replace(/^\/dev/, '')
      if (pathname === '/api/admin/relogin/templates')
        return respond(route, [template])
      if (pathname === '/api/admin/relogin/templates/save') {
        const body = request.postDataJSON()
        saves.push(body)
        assert.equal(body.selection.revision, template.revision)
        template = { ...template, revision: template.revision + 1, config: structuredClone(body.config) }
        return respond(route, template)
      }
      if (pathname === '/api/admin/accounts/apply-template') {
        const body = request.postDataJSON()
        applications.push(body)
        assert.deepEqual(Object.keys(body).sort(), ['accountIds', 'template'])
        assert.equal(body.template.revision, template.revision)
        for (const account of accounts.filter(row => body.accountIds.includes(row.id))) {
          if (template.config.modelAccess)
            account.modelAccess = structuredClone(template.config.modelAccess)
        }
        return respond(route, { accountIds: body.accountIds, configRevision: applications.length + 1 })
      }
      if (pathname === '/api/admin/accounts') {
        return respond(route, {
          items: accounts,
          page: { page: 1, pageSize: 20, total: accounts.length, totalPages: 1 },
          summary: { total: accounts.length, normal: accounts.length, error: 0, rateLimited: 0, disabled: 0, quotaExhausted: 0 },
        })
      }
      if (pathname === '/api/admin/quality-ops/models') {
        const body = request.postDataJSON()
        assert.deepEqual(Object.keys(body), ['page'], 'template directory must not probe a specific account')
        knownCatalogPages.push(body.page)
        if (failKnownCatalog)
          return route.fulfill({ status: 503, json: { code: 503, message: 'synthetic catalog unavailable', data: null } })
        return respond(route, {
          models: (body.page === 1 ? ['model-a', 'model-b'] : ['model-a', 'model-c']).map(id => ({ id, name: id, reasoningEfforts: null })),
          nextPage: body.page === 1 ? 2 : null,
          matchedAccounts: 100,
          knownAccounts: 99,
          failedAccounts: 1,
        })
      }
      if (pathname.startsWith('/api/admin/accounts/models')) {
        catalogRequests += 1
        return respond(route, { models: [] })
      }
      if (pathname === '/api/admin/accounts/import-tasks' && request.method() === 'GET')
        return respond(route, { items: [] })
      if (pathname === '/api/admin/proxies')
        return respond(route, { items: [], page: { page: 1, pageSize: 200, total: 0, totalPages: 0 } })
      if (pathname === '/api/admin/ipv6-egress')
        return respond(route, { revision: 1, defaultMode: 'unchanged', addresses: [], accountOverrides: {}, fixedBindings: {} })
      return route.continue()
    })
    const dialog = page.getByRole('dialog')
    const menu = page.getByLabel('账号模板菜单', { exact: true })
    async function manage() {
      await page.getByRole('button', { name: '账号模板', exact: true }).click()
      await menu.getByRole('button', { name: '管理模板', exact: true }).click()
      await dialog.getByRole('button', { name: '编辑模板', exact: true }).click()
      await dialog.getByRole('radiogroup', { name: '模型限制模式', exact: true }).waitFor()
    }
    async function saveAndClose() {
      await dialog.getByRole('button', { name: '保存模板', exact: true }).click()
      await dialog.getByText(template.config.name, { exact: true }).waitFor()
      await dialog.getByRole('button', { name: '关闭', exact: true }).last().click()
    }
    async function apply() {
      await page.getByRole('button', { name: '账号模板', exact: true }).click()
      await menu.getByRole('button', { name: `应用模板：${template.config.name}`, exact: true }).click()
      await menu.waitFor({ state: 'hidden' })
    }
    await page.goto(`${base}/accounts`)
    const row = page.locator(`tbody tr[data-row-key="${accounts[0].id}"]`)
    await row.waitFor()
    await row.getByRole('checkbox', { name: '选择账号', exact: true }).locator('..').click()
    await apply()
    assert.deepEqual(accounts[0].modelAccess, { mode: 'denylist', models: ['model-original'] })
    await manage()
    assert.equal(await dialog.getByRole('radio', { name: '保留', exact: true }).isChecked(), true)
    await dialog.getByRole('radio', { name: '白名单', exact: true }).click()
    await dialog.getByRole('button', { name: '保存模板', exact: true }).click()
    await dialog.getByRole('alert').filter({ hasText: '请至少选择或添加一个模型' }).waitFor()
    assert.equal(saves.length, 0)
    const input = dialog.getByRole('textbox', { name: '搜索或添加模型', exact: true })
    await input.fill('model-*')
    await input.press('Enter')
    await dialog.getByRole('alert').filter({ hasText: '请输入有效的精确模型 ID' }).waitFor()
    await dialog.getByRole('alert').filter({ hasText: '模型列表加载失败' }).waitFor()
    failKnownCatalog = false
    await input.fill('')
    await dialog.getByRole('button', { name: '重试加载模型', exact: true }).click()
    await dialog.getByRole('checkbox', { name: 'model-a', exact: true }).locator('..').click()
    await input.fill('model-b')
    assert.equal(await dialog.getByRole('checkbox', { name: 'model-a', exact: true }).count(), 0)
    assert.equal(await dialog.getByRole('checkbox', { name: 'model-b', exact: true }).count(), 1)
    await input.fill('')
    await dialog.getByRole('button', { name: '加载更多模型', exact: true }).click()
    await dialog.getByRole('checkbox', { name: 'model-c', exact: true }).locator('..').click()
    assert.equal(await dialog.getByRole('checkbox', { name: 'model-a', exact: true }).count(), 1, 'deduplicate paged models')
    assert.equal(await dialog.getByRole('button', { name: '加载更多模型', exact: true }).count(), 0)
    await dialog.getByText('部分账号的模型目录暂不可用，已显示读取成功的模型，仍可手动添加。', { exact: true }).waitFor()
    const longModel = `model-${'x'.repeat(90)}`
    await input.fill(longModel)
    await input.press('Enter')
    assert.equal(await dialog.getByRole('checkbox', { name: 'model-a', exact: true }).isChecked(), true)
    await saveAndClose()
    const policy = { mode: 'allowlist', models: ['model-a', 'model-c', longModel] }
    assert.deepEqual(saves.at(-1).config.modelAccess, policy)
    await apply()
    assert.deepEqual(accounts[0].modelAccess, policy)
    await manage()
    assert.equal(await dialog.getByRole('checkbox', { name: 'model-a', exact: true }).isChecked(), true)
    failKnownCatalog = true
    await dialog.getByRole('button', { name: '刷新模型', exact: true }).click()
    await dialog.getByRole('alert').filter({ hasText: '模型列表加载失败' }).waitFor()
    assert.equal(await dialog.getByRole('checkbox', { name: longModel, exact: true }).isChecked(), true)
    failKnownCatalog = false
    await dialog.getByRole('button', { name: '重试加载模型', exact: true }).click()
    await dialog.getByRole('alert').filter({ hasText: '模型列表加载失败' }).waitFor({ state: 'hidden' })
    assert.equal(knownCatalogPages.at(-1), 1, 'retry the failed refresh page')
    await page.getByRole('button', { name: '关闭成功通知', exact: true }).first().waitFor({ state: 'hidden' })
    for (const theme of ['light', 'dark']) {
      await page.setViewportSize({ width: 1440, height: 1000 })
      if (await page.evaluate(() => document.documentElement.dataset.theme) !== theme) {
        await page.getByRole('button', { name: theme === 'dark' ? '切换暗黑模式' : '切换浅色模式', exact: true })
          .evaluate(button => button.click())
      }
      await page.waitForFunction(theme => document.documentElement.dataset.theme === theme, theme)
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: width < 500 ? 844 : 1000 })
        await page.evaluate(() => document.fonts.ready)
        assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1), `dialog overflow at ${width}`)
        const bounds = await dialog.boundingBox()
        assert.ok(bounds.x >= -1 && bounds.x + bounds.width <= width + 1)
        await page.screenshot({ path: `${output}/template-${theme}-${width}.png`, fullPage: true })
      }
    }
    await page.setViewportSize({ width: 1440, height: 1000 })
    await dialog.getByRole('button', { name: '返回', exact: true }).click()
    await dialog.getByRole('button', { name: '关闭', exact: true }).last().click()
    for (const [label, mode] of [['黑名单', 'denylist'], ['不限制', 'all'], ['保留', undefined]]) {
      await manage()
      await dialog.getByRole('radio', { name: label, exact: true }).click()
      if (mode === 'denylist') {
        await dialog.getByRole('checkbox', { name: 'model-b', exact: true }).locator('..').click()
        policy.models.push('model-b')
      }
      await saveAndClose()
      if (mode)
        assert.deepEqual(saves.at(-1).config.modelAccess, { mode, models: mode === 'all' ? [] : policy.models })
      else
        assert.equal('modelAccess' in saves.at(-1).config, false)
      await apply()
      assert.deepEqual(accounts[0].modelAccess, mode === 'denylist' ? { mode, models: policy.models } : { mode: 'all', models: [] })
    }
    await manage()
    await dialog.getByRole('radio', { name: '白名单', exact: true }).click()
    await input.fill('model-unsaved')
    await input.press('Enter')
    await dialog.getByRole('button', { name: '返回', exact: true }).click()
    await dialog.getByRole('button', { name: '编辑模板', exact: true }).click()
    assert.equal(await dialog.getByRole('radio', { name: '保留', exact: true }).isChecked(), true)
    assert.equal(saves.length, 4, 'cancel must not submit a mutation')
    await dialog.getByRole('button', { name: '返回', exact: true }).click()
    await dialog.getByRole('button', { name: '新建模板', exact: true }).click()
    assert.equal(await dialog.getByRole('radio', { name: '保留', exact: true }).isChecked(), true)
    await page.goto(`${base}/relogin`)
    await page.getByRole('button', { name: '账号模板', exact: true }).click()
    await dialog.getByRole('button', { name: '编辑模板', exact: true }).click()
    assert.equal(await dialog.getByRole('radio', { name: '保留', exact: true }).isChecked(), true)
    assert.equal(catalogRequests, 0, 'unbound templates must not probe account model catalogs')
    assert.ok(knownCatalogPages.includes(1) && knownCatalogPages.includes(2), 'use known model directory pages')
    assert.deepEqual(errors, [])
    process.stdout.write(`${JSON.stringify({ result: 'passed', saves: saves.length, applications: applications.length, layouts: 6, catalogRequests, knownCatalogPages })}\n`)
  }
  finally {
    await browser?.close()
    server.kill('SIGTERM')
    if (server.exitCode === null)
      await new Promise(resolve => server.once('exit', resolve))
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
