import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { accounts, reloginEntries } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const port = '5191'
  const server = spawn(process.execPath, ['tests/relogin-count-preview.mjs'], {
    env: { ...process.env, QA_PORT: port },
    stdio: 'ignore',
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-excel-ui'
  await mkdir(output, { recursive: true })
  let browser
  try {
    for (let i = 0; i < 100; i++) {
      try {
        if ((await fetch(`http://127.0.0.1:${port}/accounts`)).ok)
          break
      }
      catch {}
      await new Promise(resolve => setTimeout(resolve, 100))
    }
    browser = await chromium.launch({ headless: true })
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, reducedMotion: 'reduce' })
    const errors = []
    async function dismissNotices() {
      const notices = page.getByRole('button', { name: '关闭成功通知', exact: true })
      await notices.evaluateAll(buttons => buttons.forEach(button => button.click()))
    }
    page.on('pageerror', error => errors.push(error.message))
    let enabled = false
    let excelModels = ['gpt-5.6-sol']
    let followGlobal = false
    const templates = []
    const pushes = []
    const patches = []
    const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
    await page.route('**/dev/api/admin/ipv6-egress', route => fulfill(route, {
      revision: 1,
      defaultMode: 'unchanged',
      addresses: [],
      accountOverrides: {},
      fixedBindings: {},
    }))
    await page.route('**/dev/api/admin/proxies?*', route => fulfill(route, {
      items: [],
      page: { page: 1, pageSize: 200, total: 0, totalPages: 0 },
    }))
    await page.route('**/dev/api/admin/accounts?*', route => fulfill(route, {
      items: accounts.map((account, index) => ({
        ...account,
        responsesUpstream: index === 0 && enabled ? 'excel' : 'codex',
        excelModels: index === 0 ? excelModels : ['gpt-5.6-sol'],
        excelModelsFollowGlobal: index === 0 ? followGlobal : true,
        effectiveExcelModels: index === 0 && !followGlobal ? excelModels : ['gpt-5.6-sol', 'gpt-6-astra'],
        turnStateInjectionEnabled: false,
        turnState: null,
      })),
      page: { page: 1, pageSize: 20, total: 3, totalPages: 1 },
      summary: { total: 3, normal: 3, error: 0, disabled: 0, rateLimited: 0, quotaExhausted: 0 },
    }))
    await page.route('**/dev/api/admin/accounts/update', (route) => {
      const body = route.request().postDataJSON()
      patches.push(body)
      if (body.excelModels)
        excelModels = body.excelModels
      if (body.excelModelsFollowGlobal !== undefined)
        followGlobal = body.excelModelsFollowGlobal
      return fulfill(route, null)
    })
    await page.route('**/dev/api/admin/relogin/templates', route => fulfill(route, templates))
    await page.route('**/dev/api/admin/relogin/templates/save', (route) => {
      const body = route.request().postDataJSON()
      const row = { id: 'excel-template', revision: 1, config: body.config }
      templates.push(row)
      return fulfill(route, row)
    })
    await page.route('**/dev/api/admin/relogin', route => fulfill(route, {
      settings: { concurrency: 1, paused: false, maxRetries: 2, retryIntervalMinutes: 5 },
      items: [{ ...reloginEntries[0], poolAccountIds: [], poolStatus: 'not_in_pool', message: '待推送', syncedAt: null }],
    }))
    await page.route('**/dev/api/admin/relogin/push', (route) => {
      pushes.push(route.request().postDataJSON())
      return fulfill(route, [{ id: reloginEntries[0].id, success: true, message: '已推送' }])
    })
    await page.route('**/dev/api/admin/accounts/batch-update', (route) => {
      const body = route.request().postDataJSON()
      patches.push(body)
      enabled = body.responsesUpstream === 'excel'
      return fulfill(route, null)
    })
    await page.goto(`http://127.0.0.1:${port}/accounts`)
    const more = page.getByRole('button', { name: '更多操作', exact: true }).first()
    await more.click()
    await page.getByRole('button', { name: '开启 Excel 入口', exact: true }).click()
    await page.locator('[aria-label="Excel 入口"]').first().waitFor()
    assert.deepEqual(patches[0], { accountIds: [accounts[0].id], responsesUpstream: 'excel' })
    await page.locator('button[title="展开统计"]').first().click()
    const panel = page.locator('dl').filter({ hasText: '生成入口' })
    await panel.waitFor()
    assert.equal(await page.locator('[data-account-turn-state-panel]').count(), 0)
    for (const theme of ['light', 'dark']) {
      await page.setViewportSize({ width: 1440, height: 900 })
      if (await page.locator('html').getAttribute('data-theme') !== theme) {
        await page.getByRole('button', { name: theme === 'dark' ? '切换暗黑模式' : '切换浅色模式', exact: true }).click()
      }
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 900 })
        await panel.scrollIntoViewIfNeeded()
        assert.ok(await panel.evaluate(element => element.scrollWidth <= element.clientWidth))
        if (width < 640)
          assert.ok(await panel.evaluate(element => element.getBoundingClientRect().width <= window.innerWidth - 88))
        await page.screenshot({ path: `${output}/excel-${theme}-${width}.png` })
      }
    }
    await page.setViewportSize({ width: 1440, height: 900 })
    await more.click()
    await page.getByRole('button', { name: '关闭 Excel 入口', exact: true }).click()
    await page.locator('[aria-label="Excel 入口"]').waitFor({ state: 'detached' })
    assert.deepEqual(patches[1], { accountIds: [accounts[0].id], responsesUpstream: 'codex' })
    await page.getByRole('button', { name: '编辑账号', exact: true }).first().click()
    const dialog = page.getByRole('dialog')
    const models = dialog.getByRole('textbox', { name: 'Excel 模型', exact: true })
    await models.fill('gpt-5.6-sol, gpt-6-astra')
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 900 })
      await models.scrollIntoViewIfNeeded()
      assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth))
      await page.screenshot({ path: `${output}/excel-model-edit-${width}.png` })
    }
    await page.setViewportSize({ width: 1440, height: 900 })
    await dialog.getByRole('button', { name: '保存账号设置', exact: true }).click()
    await dialog.waitFor({ state: 'detached' })
    assert.deepEqual(patches.at(-1).excelModels, ['gpt-5.6-sol', 'gpt-6-astra'])
    assert.equal(patches.at(-1).excelModelsFollowGlobal, false)
    assert.equal(Object.hasOwn(patches.at(-1), 'responsesUpstream'), false)
    await page.getByRole('button', { name: '编辑账号', exact: true }).first().click()
    await dialog.getByRole('combobox', { name: 'Excel 模型来源' }).click()
    await page.getByRole('option', { name: '跟随全局', exact: true }).click()
    assert.equal(await models.count(), 0)
    await dialog.getByRole('button', { name: '保存账号设置', exact: true }).click()
    await dialog.waitFor({ state: 'detached' })
    assert.equal(patches.at(-1).excelModelsFollowGlobal, true)
    assert.equal('excelModels' in patches.at(-1), false)

    await page.getByRole('button', { name: '账号模板', exact: true }).click()
    await page.getByRole('button', { name: '管理模板', exact: true }).click()
    await dialog.getByRole('button', { name: '新建模板', exact: true }).click()
    await dialog.getByRole('textbox', { name: '模板名称', exact: true }).fill('Excel template')
    await dialog.getByRole('switch', { name: '切换 Excel 入口', exact: true }).locator('..').click()
    const autoDisable = dialog.getByRole('switch', { name: 'Excel 遇到 HTTP 403 自动关闭', exact: true })
    await autoDisable.locator('..').click()
    await dialog.getByRole('combobox', { name: 'Excel 模型来源' }).click()
    await page.getByRole('option', { name: '自定义', exact: true }).click()
    await models.fill('gpt-6-astra')
    await dismissNotices()
    await page.setViewportSize({ width: 390, height: 900 })
    await models.scrollIntoViewIfNeeded()
    assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth))
    await page.screenshot({ path: `${output}/excel-template-390.png` })
    await dialog.getByRole('button', { name: '保存模板', exact: true }).click()
    await dialog.getByText('Excel template', { exact: true }).waitFor()
    assert.equal(templates[0].config.responsesUpstream, 'excel')
    assert.equal(templates[0].config.excelAutoDisableOn403, true)
    assert.equal(templates[0].config.excelModelsFollowGlobal, false)
    assert.deepEqual(templates[0].config.excelModels, ['gpt-6-astra'])
    await dialog.getByRole('button', { name: '关闭', exact: true }).last().click()

    await page.setViewportSize({ width: 1440, height: 900 })
    await page.getByRole('button', { name: '导入账号', exact: true }).click()
    await dismissNotices()
    await dialog.getByRole('button', { name: /OpenAI/ }).click()
    await dialog.getByRole('checkbox', { name: '指定 Excel 设置', exact: true }).locator('..').click()
    await dialog.getByRole('switch', { name: '切换 Excel 入口' }).waitFor()
    assert.ok((await dialog.getByRole('combobox', { name: 'Excel 模型来源' }).textContent()).includes('跟随全局'))
    await page.setViewportSize({ width: 320, height: 900 })
    await dialog.getByRole('combobox', { name: 'Excel 模型来源' }).scrollIntoViewIfNeeded()
    assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth))
    await page.waitForFunction(() => {
      const panel = document.querySelector('[role="dialog"]')
      return panel && panel.getBoundingClientRect().bottom <= window.innerHeight
    })
    await page.screenshot({ path: `${output}/excel-import-320.png` })
    await dialog.getByRole('button', { name: '取消', exact: true }).click()

    await page.setViewportSize({ width: 1440, height: 900 })
    await page.goto(`http://127.0.0.1:${port}/relogin`)
    await page.getByRole('button', { name: '推送', exact: true }).click()
    const confirm = page.getByRole('alertdialog')
    await confirm.getByRole('checkbox', { name: '指定新账号 Excel 设置（优先于模板）' }).locator('..').click()
    await confirm.getByRole('switch', { name: '切换新账号 Excel 入口' }).locator('..').click()
    await dismissNotices()
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 900 })
      assert.ok(await confirm.evaluate(element => element.scrollWidth <= element.clientWidth))
      await page.screenshot({ path: `${output}/excel-push-${width}.png` })
    }
    await confirm.getByRole('button', { name: '确认', exact: true }).click()
    await confirm.waitFor({ state: 'detached' })
    assert.deepEqual(pushes[0].newAccountExcel, { responsesUpstream: 'excel', excelModelsFollowGlobal: true, excelCacheCreationAsInput: false, excelAutoDisableOn403: false })
    let settings = {
      excelDefaultModels: ['gpt-5.6-sol', 'gpt-6-astra'],
      modelMappings: {},
      refreshMarginSeconds: 1800,
      refreshConcurrency: 4,
      maxConcurrentPerAccount: 5,
      requestIntervalMs: 25,
      rotationStrategy: 'smart',
      minCodexDesktopVersion: null,
      minCodexCliVersion: null,
      usageRetentionDays: 31,
      opsEventRetentionDays: 30,
      auditRetentionDays: 90,
    }
    await page.route('**/dev/api/admin/settings', route => fulfill(route, settings))
    await page.route('**/dev/api/admin/settings/update', (route) => {
      settings = route.request().postDataJSON()
      return fulfill(route, settings)
    })
    await page.route('**/dev/api/admin/settings/admin-api-key', route => fulfill(route, { exists: false }))
    await page.route('**/dev/api/admin/notifications/channels', route => fulfill(route, {
      smtp: { enabled: false, host: '', port: 465, security: 'tls' },
      bark: { enabled: false, serverUrl: '', level: 'active', volume: 5 },
      lastTest: null,
    }))
    await page.route('**/dev/api/admin/settings/openai-user-agent', route => fulfill(route, {
      mode: 'default',
      defaultUserAgent: 'synthetic',
      effectiveUserAgent: 'synthetic',
      verified: false,
    }))
    await page.goto(`http://127.0.0.1:${port}/settings`)
    const globalModels = page.getByRole('textbox', { name: '全局 Excel 模型', exact: true })
    await globalModels.waitFor()
    assert.equal(await globalModels.inputValue(), 'gpt-5.6-sol, gpt-6-astra')
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 900 })
      await globalModels.scrollIntoViewIfNeeded()
      assert.ok(await globalModels.evaluate(element => element.getBoundingClientRect().right <= window.innerWidth))
      await page.screenshot({ path: `${output}/excel-global-settings-${width}.png` })
    }
    await globalModels.fill('gpt-6-astra')
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await page.getByText('设置已保存', { exact: true }).waitFor()
    assert.deepEqual(settings.excelDefaultModels, ['gpt-6-astra'])
    assert.equal(await page.getByText('Local fixture: unsupported operation', { exact: true }).count(), 0)
    assert.deepEqual(errors, [])
    process.stdout.write('Excel menu, inheritance, templates, import, relogin push and responsive layouts passed.\n')
  }
  finally {
    await browser?.close()
    server.kill('SIGTERM')
    await new Promise(resolve => server.once('exit', resolve))
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
