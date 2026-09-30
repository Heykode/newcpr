import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { accounts as samples } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const port = process.env.QA_PORT || '5261'
  const server = spawn(process.execPath, ['tests/account-relogin-preview.mjs'], { env: { ...process.env, QA_PORT: port }, stdio: 'ignore' })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-excel-recovery-ui'
  await mkdir(output, { recursive: true })
  let browser
  try {
    let ready = false
    for (let i = 0; i < 100; i++) {
      assert.equal(server.exitCode, null)
      try {
        ready = (await fetch(`http://127.0.0.1:${port}/accounts`)).ok
      }
      catch {}
      if (ready)
        break
      await new Promise(resolve => setTimeout(resolve, 100))
    }
    assert.ok(ready)
    browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH || undefined })
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
    const errors = []
    const patches = []
    const accounts = structuredClone(samples).map((account, index) => ({
      ...account,
      provider: 'openai',
      authenticationKind: 'oauth',
      enabled: index !== 0,
      responsesUpstream: 'excel',
      excelModelsFollowGlobal: true,
      excel403WarningAt: '2026-09-29T00:00:00Z',
      excelAutoDisabledAt: index === 0 ? '2026-09-29T00:00:00Z' : null,
      excelRecovery: {
        enabled: true,
        intervalMinutes: 60,
        nextProbeAt: '2026-09-30T10:00:00Z',
        lastProbeAt: '2026-09-30T09:00:00Z',
        lastModel: 'fixture-model',
        lastResult: index === 1 ? 'recovered' : 'request_failed',
        recoveredAt: index === 1 ? '2026-09-30T09:00:00Z' : null,
      },
    }))
    page.on('pageerror', error => errors.push(error.message))
    await page.route('**/*', route => new URL(route.request().url()).hostname === '127.0.0.1' ? route.continue() : route.abort())
    const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
    await page.route('**/dev/api/admin/accounts?*', route => fulfill(route, {
      items: accounts,
      page: { page: 1, pageSize: 20, total: 3, totalPages: 1 },
      summary: { total: 3, normal: 2, error: 0, disabled: 1, rateLimited: 0, quotaExhausted: 0 },
    }))
    for (const endpoint of ['update', 'batch-update']) {
      await page.route(`**/dev/api/admin/accounts/${endpoint}`, (route) => {
        const patch = route.request().postDataJSON()
        patches.push(patch)
        for (const account of accounts.filter(account => (patch.accountIds ?? [patch.accountId]).includes(account.id))) {
          if (patch.excelRecovery)
            Object.assign(account.excelRecovery, patch.excelRecovery)
        }
        return fulfill(route, { accountIds: patch.accountIds ?? [patch.accountId], configRevision: 2 })
      })
    }
    await page.goto(`http://127.0.0.1:${port}/accounts`)
    const row = index => page.locator(`tr[data-row-key="${accounts[index].id}"]`)
    const avatar = index => row(index).locator('[data-account-avatar]')
    await avatar(0).waitFor()
    assert.equal(await avatar(0).getAttribute('data-account-excel-status'), 'warning')
    assert.equal(await avatar(1).getAttribute('data-account-excel-status'), 'enabled')
    assert.match(await avatar(1).getAttribute('title'), /历史 403/)
    assert.equal(await avatar(2).getAttribute('data-account-excel-status'), 'warning', 'manual enable is not verified recovery')
    const dialog = page.getByRole('dialog')
    const toggle = () => dialog.getByRole('switch', { name: '暂停后自动探测 Excel，成功后恢复调度', exact: true })
    const interval = () => dialog.getByRole('spinbutton', { name: 'Excel 恢复探测间隔（分钟）', exact: true })
    await row(0).getByRole('button', { name: '编辑账号', exact: true }).click()
    assert.equal(await toggle().isChecked(), true)
    assert.equal(await interval().inputValue(), '60')
    await interval().fill('15')
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 })
      await interval().scrollIntoViewIfNeeded()
      assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1))
      await page.screenshot({ path: `${output}/recovery-edit-${width}.png`, fullPage: true })
    }
    await page.setViewportSize({ width: 1440, height: 1000 })
    await dialog.getByRole('button', { name: '保存账号设置', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.deepEqual(patches[0].excelRecovery, { enabled: true, intervalMinutes: 15 })
    assert.equal('responsesUpstream' in patches[0], false)
    assert.equal(patches[0].enabled, false)
    await row(0).getByRole('button', { name: '编辑账号', exact: true }).click()
    assert.equal(await interval().inputValue(), '15')
    await dialog.getByRole('button', { name: '保存账号设置', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.equal('excelRecovery' in patches[1], false)
    await page.getByRole('button', { name: '关闭成功通知', exact: true }).evaluateAll(buttons => buttons.forEach(button => button.click()))
    await row(0).getByRole('checkbox', { name: '选择账号', exact: true }).locator('..').click()
    await page.getByRole('button', { name: '批量编辑账号', exact: true }).click()
    assert.equal(await toggle().isDisabled(), true)
    await dialog.getByRole('checkbox', { name: '应用 Excel 恢复探测更改', exact: true }).locator('..').click()
    assert.equal(await toggle().isDisabled(), false)
    await toggle().locator('..').click()
    await dialog.getByRole('button', { name: /保存/ }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.deepEqual(patches[2], { accountIds: [accounts[0].id], excelRecovery: { enabled: false, intervalMinutes: 15 } })
    assert.deepEqual(errors, [])
    process.stdout.write('Excel recovery browser checks passed: edit, omission, batch opt-in, verified avatar and 1440/390/320px layouts\n')
  }
  finally {
    await browser?.close()
    if (server.exitCode === null) {
      server.kill('SIGTERM')
      await once(server, 'exit')
    }
  }
}
main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
