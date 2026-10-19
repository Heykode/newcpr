import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { accounts } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const port = process.env.QA_PORT || '5397'
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-account-quality-controls-ui'
  await mkdir(output, { recursive: true })
  const server = spawn(process.execPath, ['tests/relogin-count-preview.mjs'], { env: { ...process.env, QA_PORT: port }, stdio: 'ignore' })
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
    browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH || undefined })
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
    const errors = []
    const external = []
    page.on('pageerror', error => errors.push(error.message))
    page.on('request', (request) => {
      if (!/^(?:127\.0\.0\.1|localhost)$/.test(new URL(request.url()).hostname))
        external.push(request.url())
    })
    const now = new Date().toISOString()
    const states = Object.fromEntries(accounts.slice(0, 2).map((account, i) => [account.id, {
      ruleId: `rule-${i}`,
      revision: 1,
      enabled: true,
      running: false,
      pending: false,
      nextRunAt: now,
      lastStatus: i ? 'incorrect' : 'correct',
      lastRunAt: now,
      lastAction: i ? null : 'template_restored',
      detectionMode: 'state_probe',
    }]))
    const writes = []
    const deletions = []
    let rejectToggle = false
    let failTemplates = false
    let failMonitoring = false
    let rejectRuleId = null
    let heldWrites = null
    const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
    await page.route('**/dev/api/admin/accounts?**', route => fulfill(route, {
      items: accounts.map(account => ({ ...account, qualityMonitoring: states[account.id] ?? null })),
      page: { page: 1, pageSize: 20, total: accounts.length, totalPages: 1 },
      summary: { total: 3, normal: 3, error: 0, disabled: 0, rateLimited: 0, quotaExhausted: 0 },
    }))
    await page.route('**/dev/api/admin/accounts/delete', (route) => {
      deletions.push(route.request().postDataJSON())
      return fulfill(route, { accountIds: deletions.at(-1).accountIds, configRevision: 1 })
    })
    await page.route('**/dev/api/admin/quality-ops/**', async (route) => {
      const path = new URL(route.request().url()).pathname.split('/quality-ops/')[1]
      const body = route.request().method() === 'POST' ? route.request().postDataJSON() : null
      if (path === 'monitoring') {
        if (failMonitoring)
          return route.fulfill({ status: 503, json: { code: 503, message: '监测读取失败', data: null } })
        return fulfill(route, Object.fromEntries(body.accountIds.filter(id => states[id]).map(id => [id, states[id]])))
      }
      if (path === 'enabled') {
        writes.push(body)
        await heldWrites?.promise
        if (rejectToggle || body.id === rejectRuleId)
          return route.fulfill({ status: 409, json: { code: 409, message: '规则已更新', data: null } })
        const current = Object.values(states).find(state => state.ruleId === body.id)
        assert.equal(body.revision, current.revision)
        current.enabled = body.enabled
        current.revision++
        return fulfill(route, {})
      }
      if (path === 'templates') {
        if (failTemplates)
          return route.fulfill({ status: 503, json: { code: 503, message: '模板读取失败', data: null } })
        return fulfill(route, [{ id: 'template', revision: 3, name: '状态探针模板', config: { enabled: false, detectionMode: 'state_probe', model: 'fixture-model', intervalSeconds: 100, failureAction: 'none', autoRestore: true } }])
      }
      if (path === 'templates/apply') {
        assert.equal(body.enable, true)
        assert.equal(body.revision, 3)
        assert.equal(body.targets[0].ruleId, null)
        assert.equal(body.targets[0].revision, null)
        states[body.targets[0].accountId] = { ...states[accounts[0].id], ruleId: 'new-rule', enabled: true, lastStatus: null, lastAction: null }
        return fulfill(route, [{ accountId: body.targets[0].accountId, success: true, ruleId: 'new-rule' }])
      }
      throw new Error(`Unexpected quality request ${path}`)
    })
    const row = i => page.locator(`[data-row-key="${accounts[i].id}"]`)
    const card = i => page.locator(`[data-account-card="${accounts[i].id}"]`)
    await page.goto(`http://127.0.0.1:${port}/accounts`)
    await row(0).getByText('智商正常', { exact: true }).waitFor({ timeout: 60000 }).catch(async (error) => {
      await page.screenshot({ path: `${output}/failure.png` })
      console.error('Page errors:', errors, 'Body:', (await page.locator('body').textContent())?.slice(0, 1600))
      throw error
    })
    assert.ok(await row(0).locator('[data-quality-verdict]').evaluate(el => el.classList.contains('text-cp-success')))
    assert.ok(await row(1).locator('[data-quality-verdict]').evaluate(el => el.classList.contains('text-cp-error')))
    await row(0).getByRole('button', { name: '更多操作', exact: true }).click()
    const menu = page.getByRole('group', { name: '账号操作', exact: true })
    const buttons = await menu.getByRole('button').allTextContents()
    assert.match(buttons[0], /测试连接/)
    assert.match(buttons[1], /质量检测开关/)
    await menu.getByRole('button', { name: /质量检测开关/ }).click()
    await row(0).getByText('监测暂停', { exact: true }).waitFor()
    assert.deepEqual(writes[0], { id: 'rule-0', revision: 1, enabled: false })
    rejectToggle = true
    await row(0).getByRole('button', { name: '更多操作', exact: true }).click()
    await menu.getByRole('button', { name: /质量检测开关/ }).click()
    await page.getByText('规则已更新', { exact: true }).waitFor()
    await row(0).getByText('监测暂停', { exact: true }).waitFor()
    rejectToggle = false
    await row(0).getByRole('button', { name: '更多操作', exact: true }).click()
    await menu.getByRole('button', { name: /质量检测开关/ }).click()
    await row(0).getByText('监测中', { exact: true }).waitFor()
    failTemplates = true
    await row(2).getByRole('button', { name: '更多操作', exact: true }).click()
    await menu.getByRole('button', { name: /质量检测开关/ }).click()
    const setup = page.getByRole('dialog', { name: '开启质量检测', exact: true })
    await setup.getByRole('alert').waitFor()
    assert.equal(await setup.getByRole('button', { name: '确认开启', exact: true }).isDisabled(), true)
    failTemplates = false
    await setup.getByRole('button', { name: '刷新模板', exact: true }).click()
    await setup.getByRole('button', { name: /状态探针模板/ }).click()
    failTemplates = true
    await setup.getByRole('button', { name: '刷新模板', exact: true }).click()
    await setup.getByRole('alert').waitFor()
    assert.equal(await setup.getByRole('button', { name: /状态探针模板/ }).count(), 0)
    assert.equal(await setup.getByRole('button', { name: '确认开启', exact: true }).isDisabled(), true)
    failTemplates = false
    await setup.getByRole('button', { name: '取消', exact: true }).click()
    assert.equal(states[accounts[2].id], undefined)

    const batchTrigger = page.getByRole('button', { name: '质量检测', exact: true })
    const batchMenu = page.getByRole('group', { name: '批量质量检测', exact: true })
    const batchSwitch = async (enabled) => {
      await batchTrigger.click()
      await batchMenu.getByRole('button', { name: enabled ? '批量开启' : '批量关闭', exact: true }).click()
    }
    const clearNotices = async () => {
      await page.getByRole('button', { name: /^关闭.*通知$/ }).evaluateAll(buttons => buttons.forEach(button => button.click()))
    }
    assert.equal(await batchTrigger.count(), 0)
    for (const index of [0, 1, 2])
      await row(index).getByRole('checkbox', { name: '选择账号', exact: true }).locator('..').click()
    heldWrites = Promise.withResolvers()
    const beforePause = writes.length
    await batchSwitch(false)
    await page.waitForFunction(() => document.querySelector('button[aria-label="质量检测"]')?.disabled)
    assert.equal(await page.getByRole('button', { name: '监测模板', exact: true }).isDisabled(), true)
    assert.equal(await page.getByRole('button', { name: '删除选中 (3)', exact: true }).isDisabled(), true)
    await row(0).getByRole('button', { name: '更多操作', exact: true }).click()
    assert.equal(await menu.getByRole('button', { name: /质量检测开关/ }).isDisabled(), true)
    await page.keyboard.press('Escape')
    heldWrites.resolve()
    heldWrites = null
    await row(0).getByText('监测暂停', { exact: true }).waitFor()
    await row(1).getByText('监测暂停', { exact: true }).waitFor()
    assert.equal(writes.length - beforePause, 2)
    assert.equal(states[accounts[2].id], undefined, 'no automatic setup for missing rules')
    await page.getByText('质量检测已关闭 2 个；1 个未配置，已跳过', { exact: true }).waitFor()
    await clearNotices()

    // A mixed selection must converge on the requested state, never invert each row.
    await row(0).getByRole('button', { name: '更多操作', exact: true }).click()
    await menu.getByRole('button', { name: /质量检测开关/ }).click()
    await row(0).getByText('监测中', { exact: true }).waitFor()
    const beforeEnable = writes.length
    await batchSwitch(true)
    await row(1).getByText('监测中', { exact: true }).waitFor()
    assert.deepEqual(writes.slice(beforeEnable).map(write => write.id), ['rule-1'])
    await page.getByText(/质量检测已开启 2 个；1 个未配置，已跳过；请先应用监测模板/).waitFor()
    await clearNotices()
    failMonitoring = true
    const beforeReadFailure = writes.length
    await batchSwitch(false)
    await page.getByText('监测读取失败', { exact: true }).waitFor()
    assert.equal(writes.length, beforeReadFailure)
    failMonitoring = false
    await clearNotices()
    rejectRuleId = 'rule-1'
    await batchSwitch(false)
    await page.getByText('质量检测已关闭 1 个；1 个未配置，已跳过；1 个未完成：规则已更新', { exact: true }).waitFor()
    await row(0).getByText('监测暂停', { exact: true }).waitFor()
    await row(1).getByText('监测中', { exact: true }).waitFor()
    rejectRuleId = null
    const beforeRetry = writes.length
    await batchSwitch(false)
    await row(1).getByText('监测暂停', { exact: true }).waitFor()
    assert.deepEqual(writes.slice(beforeRetry).map(write => write.id), ['rule-1'])
    await batchSwitch(true)
    await row(0).getByText('监测中', { exact: true }).waitFor()
    await row(1).getByText('监测中', { exact: true }).waitFor()
    for (const index of [0, 1, 2])
      await row(index).getByRole('checkbox', { name: '选择账号', exact: true }).locator('..').click()
    await clearNotices()
    assert.ok(writes.every(write => Object.keys(write).sort().join(',') === 'enabled,id,revision'))

    for (const theme of ['light', 'dark']) {
      await page.setViewportSize({ width: 1440, height: 1000 })
      if (await page.locator('html').getAttribute('data-theme') !== theme) {
        await page.getByRole('button', { name: theme === 'dark' ? '切换暗黑模式' : '切换浅色模式', exact: true }).evaluate(button => button.click())
        await page.waitForFunction(value => document.documentElement.dataset.theme === value, theme)
      }
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 1000 })
        const first = width < 768 ? card(0) : row(0)
        await first.getByText('智商正常', { exact: true }).waitFor()
        assert.equal(await first.getByText('已恢复应用模板前的账号配置', { exact: false }).count(), 0)
        if (width < 768)
          assert.ok(await card(0).evaluate(el => el.scrollWidth <= el.clientWidth + 1), `card overflow ${width}`)
        const badge = first.getByRole('link', { name: /智商正常/ })
        assert.ok(await badge.evaluate(el => el.scrollWidth <= el.clientWidth + 1), `badge overflow ${width}`)
        await first.getByRole('checkbox', { name: '选择账号', exact: true }).locator('..').click()
        const toolbar = page.getByRole('group', { name: '账号筛选与操作', exact: true })
        assert.ok(await toolbar.evaluate(el => el.scrollWidth <= el.clientWidth + 1), `toolbar overflow ${width}`)
        await batchTrigger.click()
        await batchMenu.getByRole('button', { name: '批量开启', exact: true }).waitFor()
        const menuBox = await batchMenu.boundingBox()
        assert.ok(menuBox.x >= 0 && menuBox.x + menuBox.width <= width + 1, `batch menu overflow ${width}`)
        await page.screenshot({ path: `${output}/batch-quality-${theme}-${width}.png`, animations: 'disabled' })
        await batchMenu.getByRole('button', { name: '批量关闭', exact: true }).click()
        await first.getByText('监测暂停', { exact: true }).waitFor()
        await batchSwitch(true)
        await first.getByText('监测中', { exact: true }).waitFor()
        await first.getByRole('checkbox', { name: '选择账号', exact: true }).locator('..').click()
        await clearNotices()
        await page.screenshot({ path: `${output}/accounts-${theme}-${width}.png`, animations: 'disabled' })
        await first.getByRole('button', { name: '删除账号', exact: true }).click()
        const confirm = page.getByRole('alertdialog', { name: '删除账号', exact: true })
        const checkbox = confirm.getByRole('checkbox', { name: '同步删除 2FA', exact: true })
        assert.equal(await checkbox.isChecked(), false)
        assert.equal(await confirm.getByText('该邮箱其他工作区账号', { exact: false }).count(), 0)
        assert.equal(await confirm.getByText('删除后账号不再参与调度', { exact: false }).count(), 0)
        await checkbox.locator('..').click()
        await page.screenshot({ path: `${output}/delete-${theme}-${width}.png`, animations: 'disabled' })
        const box = await confirm.boundingBox()
        assert.ok(box.x >= 0 && box.x + box.width <= width + 1)
        await confirm.getByRole('button', { name: '取消', exact: true }).click()
      }
    }
    assert.equal(deletions.length, 0)
    await card(0).getByRole('button', { name: '删除账号', exact: true }).click()
    const confirm = page.getByRole('alertdialog', { name: '删除账号', exact: true })
    await confirm.getByRole('checkbox', { name: '同步删除 2FA', exact: true }).locator('..').click()
    await confirm.getByRole('button', { name: '确认删除', exact: true }).click()
    await confirm.waitFor({ state: 'hidden' })
    assert.equal(deletions[0].deleteRelogin, true)
    await card(2).getByRole('button', { name: '更多操作', exact: true }).click()
    await menu.getByRole('button', { name: /质量检测开关/ }).click()
    await setup.getByRole('button', { name: /状态探针模板/ }).click()
    await setup.getByRole('button', { name: '确认开启', exact: true }).click()
    await setup.waitFor({ state: 'hidden' })
    await card(2).getByText('等待首次检测', { exact: true }).waitFor()
    assert.equal(states[accounts[2].id].enabled, true)
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 })
      for (const index of [1, 2]) {
        const account = width < 768 ? card(index) : row(index)
        await account.getByRole('checkbox', { name: '选择账号', exact: true }).locator('..').click()
      }
      const removeSelected = page.getByRole('button', { name: '删除选中 (2)', exact: true })
      await removeSelected.click()
      const batch = page.getByRole('alertdialog', { name: '确认删除', exact: true })
      const checkbox = batch.getByRole('checkbox', { name: '同步删除 2FA', exact: true })
      assert.equal(await checkbox.isChecked(), false)
      assert.equal(await batch.getByText('该邮箱其他工作区账号', { exact: false }).count(), 0)
      await checkbox.locator('..').click()
      await page.getByRole('button', { name: '关闭成功通知', exact: true }).evaluateAll(buttons => buttons.forEach(button => button.click()))
      await page.screenshot({ path: `${output}/batch-delete-${width}.png`, animations: 'disabled' })
      const beforeCancel = deletions.length
      await batch.getByRole('button', { name: '取消', exact: true }).click()
      assert.equal(deletions.length, beforeCancel)
      await removeSelected.click()
      assert.equal(await checkbox.isChecked(), false)
      await checkbox.locator('..').click()
      await batch.getByRole('button', { name: '确认删除', exact: true }).click()
      await batch.waitFor({ state: 'hidden' })
      assert.equal(deletions.at(-1).deleteRelogin, true)
      assert.deepEqual(deletions.at(-1).accountIds, [accounts[1].id, accounts[2].id])
    }
    assert.deepEqual(errors, [])
    assert.deepEqual(external, [])
    process.stdout.write('PASS account quality controls: single/batch switches, mixed states, partial failures, read failure, mutation guards, first setup, deletion, light/dark 1440/390/320px; no external requests\n')
  }
  finally {
    await browser?.close()
    if (server.exitCode === null) {
      const stopped = once(server, 'exit')
      server.kill('SIGTERM')
      await stopped
    }
  }
}
main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
