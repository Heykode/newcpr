import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { accounts as fixtures } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH || undefined })
  const base = process.env.QA_BASE_URL || 'http://127.0.0.1:5227'
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-account-egress-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
  const accounts = structuredClone(fixtures)
  accounts[0].requestProxySource = 'mihomo'
  accounts[0].responsesUpstream = 'excel'
  accounts[0].outboundProxyEndpoint = 'http://proxy.example:8080'
  const writes = []
  const errors = []
  const templates = []
  let failEgress = false
  let egressReads = 0
  const egress = { revision: 1, defaultMode: 'unchanged', addresses: [{ id: 'ipv6-1', address: '2001:db8::1', enabled: true }], accountOverrides: {}, fixedBindings: { acct_sample_0: '2001:db8::1' } }
  page.on('pageerror', error => errors.push(error.message))
  // Intercept every API call. No request reaches an account, proxy or real backend.
  await page.route(url => /^\/(?:dev\/)?api\//.test(url.pathname), async (route) => {
    const request = route.request()
    const path = new URL(request.url()).pathname.replace(/^\/dev/, '')
    const body = request.method() === 'POST' ? request.postDataJSON() : undefined
    let data = null
    if (body && !path.endsWith('/relogin/accounts/query'))
      writes.push({ path, body })
    if (path.endsWith('/auth/status')) {
      data = { authenticated: true }
    }
    else if (path.endsWith('/system/version')) {
      data = { version: 'isolated-egress-test', buildType: 'test' }
    }
    else if (path === '/api/admin/accounts') {
      data = { items: accounts, page: { page: 1, pageSize: 20, total: 3, totalPages: 1 }, summary: { total: 3, normal: 3, error: 0, disabled: 0, rateLimited: 0, quotaExhausted: 0 } }
    }
    else if (path.endsWith('/account-groups') || path.endsWith('/accounts/import-tasks')) {
      data = { items: [], page: { page: 1, pageSize: 200, total: 0, totalPages: 0 } }
    }
    else if (path === '/api/admin/proxies') {
      data = { items: [
        { id: 'proxy-fixture', name: '测试代理', endpoint: 'http://proxy.example:8080', lastTest: { success: true } },
        { id: 'proxy-same-endpoint', name: '同地址不同认证代理', endpoint: 'http://proxy.example:8080', lastTest: { success: true } },
      ], page: { page: 1, pageSize: 200, total: 2, totalPages: 1 } }
    }
    else if (path.endsWith('/proxies/mihomo')) {
      data = { installed: true, running: true, nodeStates: [{ name: 'fixture' }] }
    }
    else if (path.endsWith('/ipv6-egress')) {
      egressReads += 1
      if (failEgress) {
        await route.fulfill({ status: 503, json: { code: 503, message: 'fixture read unavailable', data: null } })
        return
      }
      data = egress
    }
    else if (path.endsWith('/relogin/templates')) {
      data = templates
    }
    else if (path.endsWith('/templates/save')) {
      const row = { id: 'template-egress', revision: templates.length + 1, config: body.config }
      templates.splice(0, templates.length, row)
      data = row
    }
    else if (path.endsWith('/accounts/update') || path.endsWith('/accounts/batch-update')) {
      const ids = body.accountIds ?? [body.accountId]
      for (const id of ids) {
        const account = accounts.find(item => item.id === id)
        if (body.requestProxySource !== undefined)
          account.requestProxySource = body.requestProxySource
        if (body.outboundProxyId !== undefined)
          account.outboundProxyEndpoint = body.outboundProxyId ? 'http://proxy.example:8080' : null
        if (Object.hasOwn(body, 'egressMode'))
          egress.accountOverrides[id] = body.egressMode
      }
      data = { accountId: body.accountId, accountIds: body.accountIds, configRevision: 2 }
    }
    else if (path.endsWith('/relogin/enroll')) {
      data = { ids: [] }
    }
    else if (path.endsWith('/relogin')) {
      data = { settings: { paused: false, concurrency: 1 }, items: [] }
    }
    else if (path.endsWith('/relogin/accounts/query')) {
      data = []
    }
    await route.fulfill({ json: { code: 200, message: 'ok', data } })
  })
  const dialog = page.getByRole('dialog')
  const tunnel = () => dialog.getByRole('combobox', { name: '出站隧道', exact: true })
  async function choose(label) {
    await tunnel().click()
    await page.getByRole('option', { name: label, exact: true }).click()
  }
  async function ipv6(label) {
    await dialog.getByRole('combobox', { name: 'IPv6 出口策略', exact: true }).click()
    await page.getByRole('option', { name: new RegExp(label) }).click()
  }
  async function layout(name) {
    const notifications = page.getByRole('button', { name: '关闭成功通知', exact: true })
    await notifications.last().waitFor({ state: 'hidden', timeout: 10000 })
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: width < 500 ? 844 : 1000 })
      await tunnel().scrollIntoViewIfNeeded()
      const policy = dialog.getByRole('combobox', { name: 'IPv6 出口策略', exact: true })
      if (await policy.count())
        await policy.scrollIntoViewIfNeeded()
      assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1), `${name} overflow at ${width}`)
      await page.screenshot({ path: `${output}/${name}-${width}.png`, fullPage: true })
    }
    await page.setViewportSize({ width: 1440, height: 1000 })
  }
  const first = () => page.locator('tr[data-row-key="acct_sample_0"]')
  try {
    await page.goto(`${base}/accounts`)
    await first().getByRole('button', { name: '编辑账号', exact: true }).click()
    await dialog.getByText('Mihomo 运行中 · 1 个节点', { exact: true }).waitFor()
    assert.match(await tunnel().textContent(), /Mihomo 会话代理池/)
    assert.equal(await dialog.getByText(/当前出口：/).count(), 0)
    await tunnel().click()
    assert.equal(await page.getByRole('option', { name: '保持原设置', exact: true }).count(), 0)
    await page.getByRole('option', { name: 'Mihomo 会话代理池', exact: true }).click()
    assert.equal(await dialog.getByRole('button', { name: /独立保存/ }).count(), 0)
    await choose('IPv6 地址池')
    await ipv6('轮询 IPv6 · 复用连接')
    assert.equal(await dialog.getByRole('combobox', { name: '指定代理', exact: true }).count(), 0)
    await layout('edit-ipv6')
    await dialog.getByRole('button', { name: '取消未保存更改', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.deepEqual(writes.map(item => item.path), [])
    await first().getByRole('button', { name: '编辑账号', exact: true }).click()
    await choose('Mihomo 会话代理池')
    await dialog.getByText('Mihomo 运行中 · 1 个节点', { exact: true }).waitFor()
    await choose('指定代理')
    assert.match(await dialog.getByRole('combobox', { name: '指定代理', exact: true }).textContent(), /选择已保存的代理/)
    assert.equal(await dialog.getByRole('combobox', { name: 'IPv6 出口策略', exact: true }).count(), 0)
    await dialog.getByRole('combobox', { name: '指定代理', exact: true }).click()
    await page.getByRole('option', { name: /测试代理/ }).click()
    await choose('IPv6 地址池')
    await ipv6('轮询 IPv6 · 新建连接')
    await dialog.getByRole('button', { name: '保存账号设置', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.equal(writes.length, 1)
    assert.equal(writes[0].path, '/api/admin/accounts/update')
    assert.equal(writes[0].body.egressMode, 'random_ipv6_fresh')
    assert.equal(writes[0].body.outboundProxyId, '')
    assert.equal(writes[0].body.requestProxySource, 'account')
    assert.equal('responsesUpstream' in writes[0].body, false)

    // Saving and reopening reads the stored policy, without resubmitting it on unrelated edits.
    const beforeReopen = egressReads
    await first().getByRole('button', { name: '编辑账号', exact: true }).click()
    const policy = dialog.getByRole('combobox', { name: 'IPv6 出口策略', exact: true })
    await policy.waitFor()
    assert.match(await tunnel().textContent(), /IPv6 地址池/)
    assert.match(await policy.textContent(), /轮询 IPv6 · 新建连接/)
    assert.equal(egressReads, beforeReopen + 1, 'single editing does not duplicate the config GET')
    await dialog.getByRole('spinbutton', { name: '账号调度权重', exact: true }).fill('3')
    await dialog.getByRole('button', { name: '保存账号设置', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    for (const key of ['egressMode', 'outboundProxyId', 'requestProxySource'])
      assert.equal(key in writes.at(-1).body, false)

    for (const [mode, label] of [
      ['fixed_ipv6_reuse', '固定 IPv6 · 复用连接'],
      ['random_ipv6_reuse', '轮询 IPv6 · 复用连接'],
      ['fixed_ipv6_fresh', '固定 IPv6 · 新建连接'],
      ['random_ipv6_fresh', '轮询 IPv6 · 新建连接'],
    ]) {
      await first().getByRole('button', { name: '编辑账号', exact: true }).click()
      await policy.waitFor()
      await ipv6(label)
      await dialog.getByRole('button', { name: '保存账号设置', exact: true }).click()
      await dialog.waitFor({ state: 'hidden' })
      assert.equal(egress.accountOverrides.acct_sample_0, mode)
      await first().getByRole('button', { name: '编辑账号', exact: true }).click()
      await policy.waitFor()
      assert.ok((await policy.textContent()).includes(label))
      await dialog.getByRole('button', { name: '取消未保存更改', exact: true }).click()
      await dialog.waitFor({ state: 'hidden' })
    }

    for (const [mode, label] of [['direct', '服务器默认直连'], ['inherit', '跟随全局策略'], ['proxy_pool', '普通代理池'], ['proxy', '指定代理']]) {
      await first().getByRole('button', { name: '编辑账号', exact: true }).click()
      await choose(label)
      if (mode === 'proxy') {
        await dialog.getByRole('combobox', { name: '指定代理', exact: true }).click()
        await page.getByRole('option', { name: /同地址不同认证代理/ }).click()
      }
      await dialog.getByRole('button', { name: '保存账号设置', exact: true }).click()
      await dialog.waitFor({ state: 'hidden' })
      if (mode === 'inherit') {
        assert.equal(writes.at(-1).body.egressMode, null)
        egress.defaultMode = 'random_ipv6_reuse'
      }
      const before = writes.length
      await first().getByRole('button', { name: '编辑账号', exact: true }).click()
      await page.waitForFunction(text => document.querySelector('[aria-label="出站隧道"]')?.textContent.includes(text), label)
      assert.equal(await dialog.getByText(/当前出口：/).count(), 0)
      if (mode === 'proxy') {
        assert.match(await dialog.getByRole('combobox', { name: '指定代理', exact: true }).textContent(), /已绑定代理 · http:\/\/proxy.example:8080/)
        assert.equal(writes.at(-1).body.outboundProxyId, 'proxy-same-endpoint')
      }
      await dialog.getByRole('button', { name: '保存账号设置', exact: true }).click()
      await dialog.waitFor({ state: 'hidden' })
      assert.equal(writes.length, before + 1)
      for (const key of ['egressMode', 'outboundProxyId', 'requestProxySource'])
        assert.equal(key in writes.at(-1).body, false)
    }

    // A read failure leaves unknown routing blank and cannot turn it into direct/IPv6.
    accounts[0].outboundProxyEndpoint = null
    egress.accountOverrides.acct_sample_0 = null
    failEgress = true
    await page.reload()
    await first().getByRole('button', { name: '编辑账号', exact: true }).click()
    await dialog.getByRole('alert').filter({ hasText: '出口配置读取失败' }).waitFor()
    assert.match(await tunnel().textContent(), /出口读取失败/)
    assert.equal(await tunnel().isDisabled(), true)
    await dialog.getByRole('spinbutton', { name: '账号调度权重', exact: true }).fill('4')
    await dialog.getByRole('button', { name: '保存账号设置', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    for (const key of ['egressMode', 'outboundProxyId', 'requestProxySource'])
      assert.equal(key in writes.at(-1).body, false)
    failEgress = false
    await first().getByRole('button', { name: '编辑账号', exact: true }).click()
    await page.waitForFunction(() => document.querySelector('[aria-label="出站隧道"]')?.textContent.includes('跟随全局策略'))
    await dialog.getByRole('button', { name: '取消未保存更改', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })

    for (const id of ['acct_sample_0', 'acct_sample_1'])
      await page.locator(`tr[data-row-key="${id}"]`).getByRole('checkbox', { name: '选择账号', exact: true }).locator('..').click()
    await page.getByRole('button', { name: '批量编辑账号', exact: true }).click()
    const optIn = dialog.getByRole('checkbox', { name: '应用出站隧道更改', exact: true })
    assert.equal(await optIn.isChecked(), false)
    assert.equal(await tunnel().isDisabled(), true)
    await optIn.locator('..').click()
    await choose('IPv6 地址池')
    await ipv6('轮询 IPv6 · 复用连接')
    await layout('batch-ipv6')
    await dialog.getByRole('button', { name: '保存更改', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.deepEqual(writes.at(-1).body, { accountIds: ['acct_sample_0', 'acct_sample_1'], requestProxySource: 'account', outboundProxyId: '', egressMode: 'random_ipv6_reuse' })

    await page.getByRole('button', { name: '账号模板', exact: true }).click()
    await page.getByRole('button', { name: '管理模板', exact: true }).click()
    await dialog.getByRole('button', { name: '新建模板', exact: true }).click()
    await dialog.getByRole('textbox', { name: '模板名称', exact: true }).fill('统一出口测试')
    assert.match(await tunnel().textContent(), /保持原设置/)
    await choose('跟随全局策略')
    await layout('template-global')
    await dialog.getByRole('button', { name: '保存模板', exact: true }).click()
    await dialog.getByText('统一出口测试', { exact: true }).waitFor()
    assert.equal(templates[0].config.egressMode, null)
    assert.equal(templates[0].config.preserveOutboundProxy, false)
    await dialog.getByRole('button', { name: '编辑模板', exact: true }).click()
    await choose('保持原设置')
    await dialog.getByRole('button', { name: '保存模板', exact: true }).click()
    await dialog.getByText('统一出口测试', { exact: true }).waitFor()
    assert.equal(templates[0].config.preserveOutboundProxy, true)
    assert.equal('egressMode' in templates[0].config, false)
    await dialog.getByRole('button', { name: '关闭', exact: true }).last().click()
    await dialog.waitFor({ state: 'hidden' })

    await page.getByRole('button', { name: '导入账号', exact: true }).click()
    await dialog.getByRole('button', { name: 'OpenAI', exact: true }).click()
    assert.match(await tunnel().textContent(), /跟随全局策略/)
    await choose('IPv6 地址池')
    await ipv6('固定 IPv6 · 复用连接')
    await layout('import-ipv6')
    await dialog.getByRole('button', { name: '继续导入', exact: true }).click()
    await dialog.getByRole('radio', { name: '2FA', exact: true }).click()
    await dialog.getByRole('textbox', { name: '2FA 账号', exact: true }).fill('fixture@example.invalid----fixture-password----JBSWY3DPEHPK3PXP')
    await dialog.getByRole('button', { name: '登录并导入', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.equal(writes.at(-1).path, '/api/admin/relogin/enroll')
    assert.equal(writes.at(-1).body.settings.egressMode, 'fixed_ipv6_reuse')
    assert.equal(writes.at(-1).body.settings.clearOutboundProxy, true)
    assert.equal(writes.at(-1).body.outboundProxyId, undefined)
    assert.deepEqual(errors, [])
    process.stdout.write(`${JSON.stringify({ writes: writes.map(item => item.path), screenshots: 12, errors })}\n`)
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
