import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir, readFile } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { accounts, reloginEntries } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const port = process.env.QA_PORT || '5192'
  const output = process.env.QA_OUTPUT_DIR || '/tmp/newcpr-dual-credentials-ui'
  await mkdir(output, { recursive: true })
  const server = spawn(process.execPath, ['tests/relogin-count-preview.mjs'], { env: { ...process.env, QA_PORT: port }, stdio: 'ignore' })
  const serverExited = new Promise(resolve => server.once('exit', resolve))
  let browser
  let page
  try {
    let ready = false
    for (let attempt = 0; attempt < 100; attempt++) {
      try {
        if ((await fetch(`http://127.0.0.1:${port}/accounts`)).ok) {
          ready = true
          break
        }
      }
      catch {}
      await new Promise(resolve => setTimeout(resolve, 100))
    }
    assert.ok(ready, 'synthetic fixture server is ready')
    browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH || undefined })
    page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
    page.setDefaultTimeout(12000)
    const errors = []
    const calls = []
    let excelEnabled = false
    let rejectToggle = false
    const excelMetadata = { revision: 7, state: 'ready', hasRefreshToken: true, accessTokenExpiresAt: '2099-01-01T00:00:00Z', nextRefreshAt: null, suspended: false, refreshAttempted: false, errorReason: null, errorMessage: null }
    const excelDocument = { schema_version: 1, type: 'cpa-bps', access_token: 'synthetic-excel-only' }
    const codexDocument = { type: 'codex', access_token: 'synthetic-codex-only' }
    const excelExport = { provider: 'openai', credentialTarget: 'excel', document: excelDocument }
    const channel = (account, target) => ({
      accountId: account.id,
      entryId: `relogin_${account.id}`,
      revision: 1,
      status: 'ready',
      message: 'fixture',
      busy: false,
      blockedReason: null,
      syncedAt: null,
      target: { credential_target: target, account_id: account.id, credential_revision: target === 'excel' ? 7 : 1, user_id: account.userId, workspace_id: account.accountId },
    })
    const list = () => accounts.map((account, index) => ({
      ...account,
      responsesUpstream: index === 2 || (index === 0 && excelEnabled) ? 'excel' : 'codex',
      hasCodexCredential: index !== 2,
      authenticationKind: index === 2 ? 'none' : 'oauth',
      excelCredential: index === 1 ? null : excelMetadata,
      excelMigrationRequired: false,
      excel403Action: 'none',
      excelIgnoreEncryptedContent: false,
      quotaLimits: {},
      quotaLimitsSupported: false,
      autoUseQuotaCredits: false,
      modelAccess: { mode: 'all', models: [] },
      turnStateInjectionEnabled: false,
      turnState: null,
    }))
    const relogin = reloginEntries.map((entry, index) => {
      const common = { ...entry, hasTotp: true, status: 'ready', poolStatus: 'pending_push', syncedAt: null, pushTargets: [{ accountId: accounts[index].id, workspaceId: accounts[index].accountId, planType: 'plus', switchWorkspace: false, available: true }] }
      return { ...common, credentialTarget: 'codex', excel: { ...common, credentialTarget: 'excel' } }
    })
    page.on('pageerror', error => errors.push(error.message))
    await page.route('**/*', async (route) => {
      const url = new URL(route.request().url())
      if (!['127.0.0.1', 'localhost'].includes(url.hostname))
        return route.abort()
      if (!url.pathname.startsWith('/dev/api/'))
        return route.continue()
      const path = url.pathname.replace('/dev', '')
      const body = route.request().method() === 'POST' ? route.request().postDataJSON() : null
      calls.push({ path, body, query: Object.fromEntries(url.searchParams) })
      const ok = data => route.fulfill({ json: { code: 200, message: 'ok', data } })
      if (path === '/api/admin/accounts')
        return ok({ items: list(), page: { page: 1, pageSize: 20, total: 3, totalPages: 1 }, summary: { total: 3, normal: 3, error: 0, disabled: 0, rateLimited: 0, quotaExhausted: 0 } })
      if (path === '/api/admin/accounts/batch-update' || path === '/api/admin/accounts/update') {
        if (rejectToggle && body.responsesUpstream) {
          rejectToggle = false
          return route.fulfill({ status: 503, json: { code: 503, message: 'Synthetic toggle rejected', data: null } })
        }
        if (body.responsesUpstream)
          excelEnabled = body.responsesUpstream === 'excel'
        return ok(null)
      }
      if (path === '/api/admin/relogin/accounts/query')
        return ok(list().map(account => ({ ...channel(account, 'codex'), excel: channel(account, 'excel') })))
      if (path === '/api/admin/relogin/accounts/queue')
        return ok(null)
      if (path === '/api/admin/accounts/models' || path === '/api/admin/accounts/excel/models')
        return ok({ models: [{ id: path.includes('/excel/') ? 'gpt-excel-fixture' : 'gpt-codex-fixture', label: path.includes('/excel/') ? 'Excel fixture' : 'Codex fixture' }] })
      if (path === '/api/admin/accounts/connection-test') {
        const events = [{ type: 'test_start', model: body.modelId, responses_upstream: body.responsesUpstream }, { type: 'content', text: 'OK' }, { type: 'test_complete', success: true }]
        return route.fulfill({ contentType: 'text/event-stream', body: events.map(event => `data: ${JSON.stringify(event)}\n\n`).join('') })
      }
      if (path === '/api/admin/accounts/export')
        return ok({ documents: [{ provider: 'openai', document: codexDocument }] })
      if (path === '/api/admin/accounts/excel/export')
        return ok(excelExport)
      if (path === '/api/admin/accounts/excel/import')
        return ok({ accountIds: [accounts[0].id], credentialTarget: 'excel' })
      if (path === '/api/admin/accounts/excel/oauth/start' || path === '/api/admin/accounts/oauth/start')
        return ok({ flowId: '00000000-0000-4000-8000-000000000001', authorizationUrl: 'https://example.invalid/synthetic-authorization', expiresAt: '2099-01-01T00:00:00Z' })
      if (path === '/api/admin/accounts/excel/refresh' || path === '/api/admin/accounts/refresh' || path === '/api/admin/accounts/recover')
        return ok(null)
      if (path === '/api/admin/relogin')
        return ok({ items: relogin, settings: { concurrency: 1, paused: false, maxRetries: 2, retryIntervalSeconds: 30 } })
      if (path === '/api/admin/relogin/queue' || path === '/api/admin/relogin/push')
        return ok(body.ids.map(id => ({ id, success: true, message: 'synthetic accepted', credentialTarget: body.credentialTarget === 'excel' ? 'excel' : 'codex' })))
      if (path === '/api/admin/relogin/export')
        return ok({ files: [{ name: 'synthetic-credentials.json', content: JSON.stringify({ documents: [excelExport] }) }], skipped: [] })
      if (path === '/api/admin/quality-ops/templates')
        return ok([])
      if (path === '/api/admin/relogin/templates')
        return ok([])
      return route.continue()
    })
    const row = index => page.locator(`tr[data-row-key="${accounts[index].id}"], [data-account-card="${accounts[index].id}"]`)
    const more = async (index, label) => {
      await row(index).getByRole('button', { name: '更多操作', exact: true }).click()
      await page.getByRole('button', { name: label, exact: true }).click()
    }
    const dismiss = async () => {
      await page.getByRole('button', { name: /关闭.*通知/ }).evaluateAll(buttons => buttons.forEach(button => button.click()))
    }
    const last = path => calls.filter(call => call.path === path).at(-1)
    await page.goto(`http://127.0.0.1:${port}/accounts`)
    await row(0).getByText('已有 Codex 凭据', { exact: true }).waitFor()
    await row(0).getByText('已有 Excel 凭据', { exact: true }).waitFor()
    assert.equal(await row(1).getByText('已有 Excel 凭据', { exact: true }).count(), 0)
    assert.equal(await row(2).getByText('已有 Codex 凭据', { exact: true }).count(), 0)
    process.stdout.write('Credential badges verified\n')

    for (const target of ['codex', 'excel']) {
      if (target === 'excel') {
        rejectToggle = true
        await more(0, '开启 Excel 入口')
        await page.getByText('Synthetic toggle rejected', { exact: true }).waitFor()
        assert.equal(excelEnabled, false)
        await dismiss()
        await more(0, '开启 Excel 入口')
        await row(0).locator('[data-account-excel-mark]').waitFor()
      }
      await more(0, '测试连接')
      const dialog = page.getByRole('dialog')
      await dialog.getByText(`${target === 'excel' ? 'Excel' : 'Codex'} 入口`, { exact: true }).waitFor()
      const tested = page.waitForResponse(response => response.url().endsWith('/accounts/connection-test'))
      await dialog.getByRole('button', { name: '开始测试', exact: true }).click()
      await tested
      assert.equal(last('/api/admin/accounts/connection-test').body.responsesUpstream, target)
      assert.equal(last('/api/admin/accounts/connection-test').body.modelId, `gpt-${target}-fixture`)
      await dialog.getByRole('button', { name: '关闭', exact: true }).last().click()
      await dialog.waitFor({ state: 'detached' })
      await dismiss()
    }
    process.stdout.write('Route toggle and connection tests verified\n')

    await more(0, '重登 Excel')
    await page.waitForFunction(() => document.body.textContent.includes('已加入重登队列'))
    assert.equal(last('/api/admin/relogin/accounts/queue').body.target.credential_target, 'excel')
    await dismiss()
    await more(0, '重登两种凭据')
    await page.waitForFunction(() => document.body.textContent.includes('已加入重登队列'))
    const queued = last('/api/admin/relogin/accounts/queue').body
    assert.equal(queued.target.credential_target, 'codex')
    assert.equal(queued.additionalTarget.credential_target, 'excel')
    await dismiss()

    await more(0, '重新授权')
    let dialog = page.getByRole('dialog')
    await dialog.getByRole('radio', { name: '账号文件', exact: true }).click()
    await page.setViewportSize({ width: 320, height: 1000 })
    assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
    await page.screenshot({ path: `${output}/reauthorize-320.png` })
    await page.setViewportSize({ width: 1440, height: 1000 })
    await dialog.getByRole('textbox', { name: 'Excel 凭据文件', exact: true }).fill(JSON.stringify(excelDocument))
    await dialog.getByRole('button', { name: '更新 Excel 凭据', exact: true }).click()
    await dialog.waitFor({ state: 'detached' })
    assert.deepEqual(last('/api/admin/accounts/excel/import').body, { data: excelDocument, accountId: accounts[0].id, expectedRevision: 7 })
    await dismiss()
    process.stdout.write('Scoped relogin and Excel replacement verified\n')

    await row(0).getByRole('checkbox', { name: '选择账号', exact: true }).locator('..').click()
    await page.getByRole('button', { name: '批量操作', exact: true }).click()
    await page.getByRole('button', { name: '导出选中 (1)', exact: true }).click()
    let confirm = page.getByRole('alertdialog')
    await confirm.getByRole('radio', { name: '全部', exact: true }).click()
    const downloadPromise = page.waitForEvent('download')
    await confirm.getByRole('button', { name: '确认导出', exact: true }).click()
    const download = await downloadPromise
    const exported = JSON.parse(await readFile(await download.path(), 'utf8'))
    assert.equal(exported.documents.length, 2)
    assert.deepEqual(exported.documents.find(document => document.credentialTarget === 'excel'), excelExport)
    assert.equal(JSON.stringify(exported).includes('totp'), false)
    await confirm.waitFor({ state: 'detached' })
    await dismiss()
    await row(0).getByRole('checkbox', { name: '选择账号', exact: true }).locator('..').click()

    await page.getByRole('button', { name: '导入账号', exact: true }).click()
    dialog = page.getByRole('dialog')
    await dialog.getByRole('button', { name: 'Codex', exact: true }).click()
    await dialog.getByRole('button', { name: '继续导入', exact: true }).click()
    await dialog.getByRole('radio', { name: 'Excel', exact: true }).click()
    assert.equal(await dialog.getByRole('radio', { name: 'AT', exact: true }).count(), 0)
    await dialog.getByRole('radio', { name: '账号文件', exact: true }).click()
    await dialog.getByRole('textbox', { name: 'Excel 凭据文件', exact: true }).fill(JSON.stringify(excelDocument))
    await dialog.getByRole('button', { name: '保存 Excel 凭据', exact: true }).click()
    await dialog.waitFor({ state: 'detached' })
    assert.equal(last('/api/admin/accounts/excel/import').body.accountId, undefined)
    await dismiss()
    process.stdout.write('Typed export and Excel import verified\n')

    await page.getByRole('button', { name: '编辑账号', exact: true }).first().click()
    dialog = page.getByRole('dialog')
    assert.equal(await dialog.getByRole('textbox', { name: 'Excel 模型', exact: true }).count(), 0)
    assert.equal(await dialog.getByRole('combobox', { name: 'Excel 模型来源', exact: true }).count(), 0)
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 })
      assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
      await page.screenshot({ path: `${output}/editor-${width}.png` })
    }
    await dialog.getByRole('button', { name: '取消未保存更改', exact: true }).click()
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 })
      await row(0).getByText('已有 Excel 凭据', { exact: true }).waitFor()
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1))
      await page.screenshot({ path: `${output}/accounts-${width}.png` })
    }

    await page.setViewportSize({ width: 1440, height: 1000 })
    await page.goto(`http://127.0.0.1:${port}/relogin`)
    const reloginRow = page.locator(`tr[data-row-key="${relogin[0].id}"]`)
    await reloginRow.getByRole('button', { name: '推送', exact: true }).click()
    confirm = page.getByRole('alertdialog')
    await confirm.getByRole('radio', { name: '全部', exact: true }).click()
    await confirm.getByRole('button', { name: '确认', exact: true }).click()
    await confirm.waitFor({ state: 'detached' })
    assert.equal(last('/api/admin/relogin/push').body.credentialTarget, 'all')
    await dismiss()
    await reloginRow.getByRole('button', { name: '重登', exact: true }).click()
    confirm = page.getByRole('alertdialog')
    await confirm.getByRole('radio', { name: 'Excel', exact: true }).click()
    await confirm.getByRole('button', { name: '确认', exact: true }).click()
    await confirm.waitFor({ state: 'detached' })
    assert.equal(last('/api/admin/relogin/queue').body.credentialTarget, 'excel')
    await dismiss()
    await page.getByRole('radiogroup', { name: '凭据入口', exact: true }).getByRole('radio', { name: 'Excel', exact: true }).click()
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 })
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1))
      await page.screenshot({ path: `${output}/relogin-${width}.png` })
    }
    assert.deepEqual(errors, [])
    process.stdout.write(`Dual credential browser QA passed; screenshots: ${output}\n`)
  }
  catch (error) {
    if (page) {
      await page.screenshot({ path: `${output}/failure.png`, fullPage: true }).catch(() => {})
      process.stderr.write(`${JSON.stringify(await page.locator('tr[data-row-key]').evaluateAll(rows => rows.map(row => ({ id: row.getAttribute('data-row-key'), text: row.textContent }))).catch(() => []))}\n`)
      process.stderr.write(`${(await page.locator('body').textContent().catch(() => '') || '').slice(0, 6000)}\n`)
    }
    throw error
  }
  finally {
    await browser?.close()
    server.kill('SIGTERM')
    await serverExited
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
