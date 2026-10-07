import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const port = process.env.QA_PORT || '5327'
  const base = `http://127.0.0.1:${port}`
  const output = process.env.QA_OUTPUT_DIR || '/tmp/newcpr-import-template-qa'
  await mkdir(output, { recursive: true })
  const server = spawn(process.execPath, ['tests/relogin-count-preview.mjs'], { env: { ...process.env, QA_PORT: port }, stdio: 'ignore' })
  let browser
  try {
    let ready = false
    for (let i = 0; i < 100; i++) {
      assert.equal(server.exitCode, null, 'isolated server must own its port')
      try {
        if ((await fetch(`${base}/accounts`)).ok) {
          ready = true
          break
        }
      }
      catch {}
      await new Promise(resolve => setTimeout(resolve, 100))
    }
    assert.ok(ready)
    browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH || undefined })
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
    const errors = []
    const submissions = []
    const groupId = 'grp_00000000000000000000000000000001'
    const template = {
      id: 'template-import',
      revision: 3,
      config: {
        name: '导入预填模板',
        enabled: false,
        concurrencyLimit: 4,
        weight: 9,
        groupIds: [groupId],
        outboundProxyId: 'proxy-template',
        preserveOutboundProxy: false,
        requestProxySource: 'account',
        egressMode: 'unchanged',
        responsesUpstream: 'excel',
        excelModelsFollowGlobal: true,
        excelCacheCreationAsInput: false,
        excelIgnoreEncryptedContent: true,
        excel403Action: 'disable_excel',
      },
    }
    const respond = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
    page.on('pageerror', error => errors.push(error.message))
    page.on('response', (response) => {
      if (new URL(response.url()).pathname.startsWith('/dev/api/') && response.status() >= 400)
        errors.push(`${response.status()} ${new URL(response.url()).pathname}`)
    })
    await page.route('**/dev/api/**', async (route) => {
      const request = route.request()
      const path = new URL(request.url()).pathname.replace(/^\/dev/, '')
      if (path === '/api/admin/relogin/templates')
        return respond(route, [template])
      if (path === '/api/admin/accounts/reset-credits/cache' || path === '/api/admin/accounts/reset-credits/batches')
        return respond(route, [])
      if (path === '/api/admin/account-groups/monitor')
        return respond(route, { items: [], generatedAt: new Date().toISOString(), refreshing: false, pendingGroupIds: [] })
      if (path === '/api/admin/account-groups')
        return respond(route, { items: [{ id: groupId, name: '模板分组', color: '#FFFFFFFF', enabled: true }], page: { page: 1, totalPages: 1 } })
      if (path === '/api/admin/proxies')
        return respond(route, { items: [{ id: 'proxy-template', name: '模板代理', endpoint: 'http://proxy.example:8080', lastTest: { success: true } }], page: { page: 1, totalPages: 1 } })
      if (path === '/api/admin/ipv6-egress')
        return respond(route, { revision: 1, defaultMode: 'unchanged', addresses: [], accountOverrides: {}, fixedBindings: {} })
      if (path === '/api/admin/accounts/import-tasks') {
        if (request.method() === 'GET')
          return respond(route, { items: [] })
        submissions.push(request.postDataJSON())
        return respond(route, { taskId: 'task-template', createdAt: new Date().toISOString(), finishedAt: null, stopRequested: false, total: 1, counts: { pending: 1, running: 0, succeeded: 0, failed: 0, unknown: 0, skipped: 0, importedAccounts: 0 } })
      }
      return route.continue()
    })
    const dialog = page.getByRole('dialog', { name: /^(账号设置|导入账号)$/ })
    await page.goto(`${base}/accounts`)
    await page.getByRole('button', { name: '导入账号', exact: true }).click()
    await dialog.getByRole('button', { name: 'OpenAI', exact: true }).click()
    await dialog.getByRole('combobox', { name: '导入账号模板', exact: true }).click()
    await page.getByRole('option', { name: template.config.name, exact: true }).click()
    assert.equal(await dialog.getByRole('spinbutton', { name: '账号并发限制', exact: true }).inputValue(), '4')
    assert.equal(await dialog.getByRole('spinbutton', { name: '账号调度权重', exact: true }).inputValue(), '9')
    assert.equal(await dialog.getByRole('switch', { name: '切换账号调度', exact: true }).isChecked(), false)
    assert.equal(await dialog.getByRole('switch', { name: '切换 Excel 入口', exact: true }).isChecked(), true)
    assert.equal(await dialog.getByRole('switch', { name: '忽略历史中的加密消息内容', exact: true }).isChecked(), true)
    assert.match(await dialog.getByRole('combobox', { name: '指定代理', exact: true }).textContent(), /模板代理/)
    assert.equal(await dialog.getByRole('checkbox', { name: '模板分组', exact: true }).isChecked(), true)
    for (const theme of ['light', 'dark']) {
      await page.setViewportSize({ width: 1440, height: 1000 })
      if (await page.evaluate(() => document.documentElement.dataset.theme) !== theme)
        await page.getByRole('button', { name: theme === 'dark' ? '切换暗黑模式' : '切换浅色模式', exact: true }).evaluate(button => button.click())
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: width < 500 ? 844 : 1000 })
        await dialog.getByRole('combobox', { name: '导入账号模板', exact: true }).scrollIntoViewIfNeeded()
        await page.evaluate(() => document.fonts.ready)
        assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
        const bounds = await dialog.boundingBox()
        assert.ok(bounds.x >= -1 && bounds.x + bounds.width <= width + 1)
        await page.screenshot({ path: `${output}/prefill-${theme}-${width}.png`, animations: 'disabled', fullPage: true })
      }
    }
    await page.setViewportSize({ width: 1440, height: 1000 })
    await dialog.getByRole('spinbutton', { name: '账号调度权重', exact: true }).fill('21')
    await dialog.getByRole('combobox', { name: '出站隧道', exact: true }).click()
    await page.getByRole('option', { name: '服务器默认直连', exact: true }).click()
    await dialog.getByRole('button', { name: '继续导入', exact: true }).click()
    assert.equal(await dialog.getByRole('combobox', { name: '导入账号模板', exact: true }).count(), 0)
    await dialog.getByRole('radio', { name: 'AT', exact: true }).click()
    await dialog.getByRole('textbox', { name: 'Access Token', exact: true }).fill('synthetic-import-input')
    await dialog.getByRole('button', { name: '上一步', exact: true }).click()
    assert.equal(await dialog.getByRole('spinbutton', { name: '账号调度权重', exact: true }).inputValue(), '21')
    await dialog.getByRole('button', { name: '继续导入', exact: true }).click()
    await dialog.getByRole('button', { name: '创建导入任务', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.equal(submissions.length, 1)
    const item = submissions[0].items[0]
    assert.deepEqual(item.template, { id: template.id, revision: 3 })
    assert.equal(item.templateSettingsOverride, true)
    assert.equal(item.settings.weight, 21)
    assert.equal(item.settings.concurrencyLimit, 4)
    assert.equal(item.settings.enabled, false)
    assert.equal(item.settings.excelIgnoreEncryptedContent, true)
    assert.deepEqual(item.settings.groupIds, [groupId])
    assert.equal(item.settings.clearOutboundProxy, true)
    assert.equal('outboundProxyId' in item, false)
    const tasks = page.getByRole('dialog', { name: '导入任务', exact: true })
    await tasks.getByRole('button', { name: '关闭', exact: true }).last().click()
    await tasks.waitFor({ state: 'hidden' })
    await page.getByRole('button', { name: '导入账号', exact: true }).click()
    assert.match(await dialog.getByRole('combobox', { name: '导入账号模板', exact: true }).textContent(), /不使用模板/)
    assert.equal(await dialog.getByRole('spinbutton', { name: '账号调度权重', exact: true }).inputValue(), '1')
    assert.deepEqual(errors, [])
    process.stdout.write('Passed: first-step prefill, final edited payload, back/reopen, six responsive layouts.\n')
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
