import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { accounts } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const port = process.env.QA_PORT || '5297'
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-quality-templates-ui'
  await mkdir(output, { recursive: true })
  const server = spawn(process.execPath, ['tests/relogin-count-preview.mjs'], { env: { ...process.env, QA_PORT: port }, stdio: 'ignore' })
  let browser
  try {
    let ready = false
    for (let i = 0; i < 100; i++) {
      try {
        if ((await fetch(`http://127.0.0.1:${port}/accounts`)).ok) {
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
    page.on('pageerror', error => errors.push(error.message))
    const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
    const fail = route => route.fulfill({ status: 503, json: { code: 503, message: 'fixture read unavailable', data: null } })
    const now = new Date().toISOString()
    let templates = []
    let rules = []
    let applications = 0
    let failCatalog = false
    let failMonitoring = false
    let delayMonitoring = false
    let rejectSecond = true
    let uncertain = false
    const monitor = rule => ({ ruleId: rule.id, revision: rule.revision, enabled: rule.config.enabled, running: rule.running, pending: rule.pending, nextRunAt: now, lastStatus: rule.lastStatus, lastRunAt: null, lastAction: null, sourceTemplate: rule.sourceTemplate })
    await page.route('**/dev/api/admin/accounts?**', route => fulfill(route, {
      items: accounts.map(account => ({ ...account, qualityMonitoring: rules.some(rule => rule.config.accountId === account.id) ? monitor(rules.find(rule => rule.config.accountId === account.id)) : null })),
      page: { page: 1, pageSize: 20, total: accounts.length, totalPages: 1 },
      summary: { total: accounts.length, normal: accounts.length, error: 0, rateLimited: 0, disabled: 0, quotaExhausted: 0 },
    }))
    await page.route('**/dev/api/admin/account-groups?**', route => fulfill(route, {
      items: [{ id: 'fixture-group', name: '判题分组', enabled: true, memberCount: 3 }],
      page: { page: 1, pageSize: 50, total: 1, totalPages: 1 },
    }))
    await page.route('**/dev/api/admin/quality-ops/**', async (route) => {
      const path = new URL(route.request().url()).pathname.split('/quality-ops/')[1]
      const body = route.request().method() === 'POST' ? route.request().postDataJSON() : null
      if (path === 'models')
        return fulfill(route, { models: [], nextPage: null, matchedAccounts: 3, knownAccounts: 0, failedAccounts: 0 })
      if (path === 'templates')
        return failCatalog ? fail(route) : fulfill(route, templates)
      if (path === 'templates/save') {
        assert.equal('accountId' in body.config, false)
        const saved = { ...body, id: body.id ?? 'monitor-template', revision: (body.revision ?? 0) + 1 }
        templates = [...templates.filter(template => template.id !== saved.id), saved]
        return fulfill(route, saved)
      }
      if (path === 'templates/delete') {
        templates = templates.filter(template => template.id !== body.id)
        return fulfill(route, null)
      }
      if (path === 'rules')
        return fulfill(route, rules)
      if (path === 'runs')
        return fulfill(route, [])
      if (path === 'monitoring') {
        if (delayMonitoring)
          await new Promise(resolve => setTimeout(resolve, 500))
        return failMonitoring ? fail(route) : fulfill(route, Object.fromEntries(rules.filter(rule => body.accountIds.includes(rule.config.accountId)).map(rule => [rule.config.accountId, monitor(rule)])))
      }
      if (path === 'templates/apply') {
        applications++
        if (uncertain)
          return fail(route)
        const template = templates.find(template => template.id === body.id)
        assert.equal(body.revision, template.revision)
        return fulfill(route, body.targets.map((target) => {
          const current = rules.find(rule => rule.config.accountId === target.accountId)
          assert.equal(target.ruleId, current?.id ?? null)
          assert.equal(target.revision, current?.revision ?? null)
          if (target.accountId === accounts[1].id && rejectSecond)
            return { accountId: target.accountId, ruleId: null, success: false, message: '账号已开启Excel模式，不能启用状态探针' }
          const rule = { id: current?.id ?? `rule-${target.accountId}`, revision: (current?.revision ?? 0) + 1, config: { ...template.config, accountId: target.accountId }, nextRunAt: now, running: false, pending: false, lastStatus: null, lastRunAt: null, sourceTemplate: { id: template.id, revision: template.revision, name: template.name } }
          rules = [...rules.filter(item => item.id !== rule.id), rule]
          return { accountId: target.accountId, ruleId: rule.id, success: true, message: null }
        }))
      }
      if (path === 'save') {
        const rule = rules.find(rule => rule.id === body.id)
        Object.assign(rule, { config: body.config, revision: rule.revision + 1 })
        return fulfill(route, rule)
      }
      throw new Error(`Unexpected quality operation ${path}`)
    })
    const row = index => page.locator(`tr[data-row-key="${accounts[index].id}"]`)
    async function bounds(name, locator = page.locator('body')) {
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 1000 })
        await page.waitForTimeout(100)
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), name)
        const box = await locator.boundingBox()
        assert.ok(box && box.x >= -1 && box.x + box.width <= width + 1, name)
        await page.screenshot({ path: `${output}/${name}-${width}.png`, fullPage: true, animations: 'disabled' })
      }
      await page.setViewportSize({ width: 1440, height: 1000 })
    }
    await page.goto(`http://127.0.0.1:${port}/quality-ops?tab=templates`)
    await page.getByRole('button', { name: '新建规则模板', exact: true }).click()
    const editor = page.getByRole('dialog', { name: '新建规则模板', exact: true })
    assert.equal(await editor.getByRole('combobox', { name: /^检测模型/ }).inputValue(), 'gpt-6-astra')
    assert.equal(await editor.getByRole('spinbutton', { name: '检测频率', exact: true }).inputValue(), '120')
    await editor.getByRole('textbox', { name: /^模板名称/ }).fill('每六小时检测')
    await editor.getByRole('combobox', { name: /^检测模型/ }).fill('fixture-model')
    assert.equal(await editor.getByRole('combobox', { name: /^判题模型/ }).count(), 0)
    assert.equal(await editor.getByText('被测账号', { exact: true }).count(), 0)
    await bounds('editor', editor)
    await editor.getByRole('button', { name: '保存', exact: true }).click()
    await editor.waitFor({ state: 'hidden' })
    assert.equal(templates.length, 1)
    assert.equal(templates[0].config.intervalSeconds, 120)
    assert.equal(templates[0].config.detectionMode, 'state_probe')
    assert.equal(templates[0].config.excelFailureThreshold, 2)
    assert.equal(templates[0].config.excelRecoveryThreshold, 2)
    await bounds('catalog')
    await page.getByRole('button', { name: '切换暗黑模式', exact: true }).click()
    await page.getByRole('button', { name: '切换浅色模式', exact: true }).waitFor()
    await bounds('catalog-dark')
    await page.getByRole('button', { name: '切换浅色模式', exact: true }).click()
    delete templates[0].config.intervalSeconds
    templates[0].config.cron = '15 9 * * 1-5'
    await page.reload()
    await page.getByRole('button', { name: '编辑模板：每六小时检测', exact: true }).click()
    const edit = page.getByRole('dialog', { name: '编辑规则模板', exact: true })
    await edit.getByText(/保留原定时：15 9 \* \* 1-5/).waitFor()
    await edit.getByRole('textbox', { name: /^模板名称/ }).fill('质量监测模板')
    await edit.getByRole('button', { name: '保存', exact: true }).click()
    await edit.waitFor({ state: 'hidden' })
    assert.equal(templates[0].revision, 2)
    assert.equal(templates[0].config.intervalSeconds, null)
    assert.equal(templates[0].config.cron, '15 9 * * 1-5')

    await page.goto(`http://127.0.0.1:${port}/accounts`)
    await row(0).waitFor()
    assert.equal(await row(0).getByRole('link', { name: '监测中', exact: true }).count(), 0)
    for (const index of [0, 1])
      await row(index).getByRole('checkbox', { name: '选择账号', exact: true }).locator('..').click()
    const open = () => page.getByRole('button', { name: '监测模板', exact: true }).click()
    const choose = () => page.getByRole('button', { name: '应用监测模板：质量监测模板', exact: true }).click()
    const confirmation = page.getByRole('dialog', { name: '应用监测模板', exact: true })
    delayMonitoring = true
    await open()
    await choose()
    await confirmation.getByText('正在读取所选账号的监测规则…', { exact: true }).waitFor()
    await confirmation.getByRole('button', { name: '取消', exact: true }).click()
    await page.waitForTimeout(550)
    assert.equal(await confirmation.count(), 0)
    assert.equal(applications, 0)
    delayMonitoring = false
    await open()
    await choose()
    await confirmation.getByText('将为 2 个账号新建规则，替换 0 个账号的已有规则。', { exact: true }).waitFor()
    await confirmation.getByRole('button', { name: '取消', exact: true }).click()
    assert.equal(applications, 0)
    await open()
    await choose()
    await bounds('confirmation', confirmation)
    await confirmation.getByRole('button', { name: '确认应用', exact: true }).click()
    await confirmation.getByText('已应用 1 个账号，1 个账号未应用。', { exact: true }).waitFor()
    assert.equal(rules.length, 1)
    rejectSecond = false
    await confirmation.getByRole('button', { name: '重新核对未成功项', exact: true }).click()
    await confirmation.getByText('将为 1 个账号新建规则，替换 0 个账号的已有规则。', { exact: true }).waitFor()
    await confirmation.getByRole('button', { name: '确认应用', exact: true }).click()
    await confirmation.getByRole('button', { name: '完成', exact: true }).click()
    await row(0).getByRole('link', { name: '监测中', exact: true }).waitFor()
    await row(1).getByRole('link', { name: '监测中', exact: true }).waitFor()
    assert.equal(rules[0].revision, 1, 'successful accounts must not be reapplied on retry')
    assert.ok(rules.every(rule => rule.config.excelRecoveryThreshold === 2 && rule.config.excelFailureThreshold === 2))
    await open()
    await choose()
    await confirmation.getByText('将为 0 个账号新建规则，替换 2 个账号的已有规则。', { exact: true }).waitFor()
    await confirmation.getByRole('button', { name: '取消', exact: true }).click()
    failMonitoring = true
    await open()
    await choose()
    await confirmation.getByRole('alert').waitFor()
    assert.equal(await confirmation.getByRole('button', { name: '确认应用', exact: true }).count(), 0)
    failMonitoring = false
    await confirmation.getByRole('button', { name: '重新读取', exact: true }).click()
    await confirmation.getByRole('button', { name: '确认应用', exact: true }).waitFor()
    uncertain = true
    await confirmation.getByRole('button', { name: '确认应用', exact: true }).click()
    await confirmation.getByText(/不会自动重复应用/).waitFor()
    assert.equal(await confirmation.getByRole('button', { name: '确认应用', exact: true }).count(), 0)
    await confirmation.getByRole('button', { name: '完成', exact: true }).click()
    failCatalog = true
    await open()
    await page.getByLabel('监测模板菜单', { exact: true }).getByRole('alert').waitFor()
    failCatalog = false
    await page.getByRole('button', { name: '刷新监测模板', exact: true }).click()
    await page.getByRole('button', { name: '应用监测模板：质量监测模板', exact: true }).waitFor()
    await open()
    await row(1).getByRole('link', { name: '监测中', exact: true }).click()
    await page.waitForURL(/quality-ops\?accountId=/)
    assert.ok(page.url().includes(`accountId=${accounts[1].id}`))
    const activeRule = page.locator('.quality-rule-selected')
    await activeRule.getByRole('button', { name: '暂停定时检测', exact: true }).click()
    await activeRule.getByRole('button', { name: '启用定时检测', exact: true }).waitFor()
    await page.goto(`http://127.0.0.1:${port}/accounts`)
    await row(1).getByRole('link', { name: '监测暂停', exact: true }).waitFor()
    rules[0].running = true
    await page.reload()
    await row(0).getByRole('link', { name: '检测中', exact: true }).waitFor()
    rules[0].running = false
    rules[0].pending = true
    await page.reload()
    await row(0).getByRole('link', { name: '已排队', exact: true }).waitFor()
    await bounds('accounts')
    await page.goto(`http://127.0.0.1:${port}/quality-ops?tab=templates`)
    await page.getByRole('button', { name: '删除模板：质量监测模板', exact: true }).click()
    const deletion = page.getByRole('alertdialog', { name: '删除规则模板', exact: true })
    await deletion.getByRole('button', { name: '确认', exact: true }).click()
    await deletion.waitFor({ state: 'hidden' })
    assert.equal(templates.length, 0)
    assert.equal(rules.length, 2)
    assert.deepEqual(errors, [])
    process.stdout.write(`${JSON.stringify({ templatesCreated: 1, applications, rules: rules.length, screenshots: 15, errors })}\n`)
  }
  finally {
    if (browser)
      await browser.close()
    if (server.exitCode === null) {
      server.kill('SIGTERM')
      await once(server, 'exit')
    }
  }
}
main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
