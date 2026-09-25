import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { accounts } from '../fixtures/relogin-count-data.mjs'

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
    page.on('pageerror', error => errors.push(error.message))
    let enabled = false
    let excelModels = ['gpt-5.6-sol']
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
      return fulfill(route, null)
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
    assert.equal(Object.hasOwn(patches.at(-1), 'responsesUpstream'), false)
    assert.equal(await page.getByText('Local fixture: unsupported operation', { exact: true }).count(), 0)
    assert.deepEqual(errors, [])
    process.stdout.write('Excel menu, model editing, account-local patches and responsive layouts passed.\n')
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
