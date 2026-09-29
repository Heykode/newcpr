import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { accounts } from '../fixtures/relogin-count-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const port = process.env.QA_PORT || '5298'
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-quality-groups-ui'
  await mkdir(output, { recursive: true })
  const server = spawn(process.execPath, ['tests/relogin-count-preview.mjs'], { env: { ...process.env, QA_PORT: port }, stdio: 'ignore' })
  let browser
  try {
    let ready = false
    for (let i = 0; i < 100; i++) {
      try {
        if ((await fetch(`http://127.0.0.1:${port}/quality-ops`)).ok) {
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
    const external = []
    page.on('pageerror', error => errors.push(error.message))
    await page.route('**/*', (route) => {
      if (new URL(route.request().url()).origin !== `http://127.0.0.1:${port}`) {
        external.push(route.request().url())
        return route.abort()
      }
      return route.continue()
    })
    const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
    const fail = route => route.fulfill({ status: 409, json: { code: 409, message: 'fixture revision conflict', data: null } })
    const now = new Date().toISOString()
    let groups = []
    let rules = []
    let rejectDelete = ''
    let rejectRead = false
    const deletes = []
    const groupDeletes = []
    await page.route('**/dev/api/admin/accounts?**', route => fulfill(route, {
      items: accounts,
      page: { page: 1, pageSize: 20, total: accounts.length, totalPages: 1 },
      summary: { total: accounts.length, normal: accounts.length, error: 0, rateLimited: 0, disabled: 0, quotaExhausted: 0 },
    }))
    await page.route('**/dev/api/admin/account-groups?**', route => fulfill(route, {
      items: [{ id: 'fixture-group', name: '测试分组', enabled: true, memberCount: 2 }],
      page: { page: 1, pageSize: 50, total: 1, totalPages: 1 },
    }))
    await page.route('**/dev/api/admin/quality-ops/**', async (route) => {
      const path = new URL(route.request().url()).pathname.split('/quality-ops/')[1]
      const body = route.request().method() === 'POST' ? route.request().postDataJSON() : null
      if (path === 'groups')
        return fulfill(route, groups)
      if (path === 'groups/save') {
        assert.equal('accountId' in body.config, false)
        const saved = { ...body, id: body.id ?? 'fixture-group-rule', revision: (body.revision ?? 0) + 1, ruleCount: 2, excludedCount: 0, lastSyncedAt: now }
        groups = [saved]
        if (!rules.length) {
          rules = accounts.slice(0, 2).map((account, index) => ({ id: `rule-${index}`, revision: 1, config: { ...body.config, accountId: account.id }, nextRunAt: now, running: false, pending: false, lastStatus: null, lastRunAt: null }))
        }
        return fulfill(route, { group: saved, sync: { created: body.id ? 0 : 2, updated: body.id ? 2 : 0, failed: 0 } })
      }
      if (path === 'groups/delete') {
        groupDeletes.push(body)
        groups = []
        if (body.deleteRules)
          rules = []
        return fulfill(route, null)
      }
      if (path === 'rules')
        return rejectRead ? fail(route) : fulfill(route, rules)
      if (path === 'templates' || path === 'runs')
        return fulfill(route, [])
      if (path === 'delete') {
        deletes.push(body)
        if (body.id === rejectDelete)
          return fail(route)
        const rule = rules.find(rule => rule.id === body.id)
        assert.equal(body.revision, rule.revision)
        rules = rules.filter(rule => rule.id !== body.id)
        return fulfill(route, null)
      }
      throw new Error(`Unexpected quality operation ${path}`)
    })
    async function bounds(name, locator = page.locator('body')) {
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 1000 })
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), name)
        assert.ok(await locator.evaluate(element => element.scrollWidth <= element.clientWidth + 1), name)
        await page.screenshot({ path: `${output}/${name}-${width}.png`, fullPage: true, animations: 'disabled' })
      }
      await page.setViewportSize({ width: 1440, height: 1000 })
    }
    await page.goto(`http://127.0.0.1:${port}/quality-ops?tab=groups`)
    await page.getByText('暂无分组规则', { exact: true }).waitFor()
    await page.getByRole('button', { name: '新建分组规则', exact: true }).click()
    const editor = page.getByRole('dialog', { name: '新建分组规则', exact: true })
    await editor.getByRole('textbox', { name: /^分组规则名称/ }).fill('自动监测测试组')
    await editor.getByRole('group', { name: /^被测分组/ }).getByRole('radio', { name: '测试分组', exact: true }).check()
    await editor.getByRole('checkbox', { name: '正常', exact: true }).locator('..').click()
    await editor.getByRole('textbox', { name: /^检测模型/ }).fill('fixture-model')
    await editor.getByRole('group', { name: /^判题账号分组/ }).getByRole('radio', { name: '测试分组', exact: true }).check()
    await editor.getByRole('textbox', { name: /^判题模型/ }).fill('fixture-judge')
    await bounds('group-editor', editor)
    await editor.getByRole('button', { name: '保存', exact: true }).click()
    await editor.waitFor({ state: 'hidden' })
    assert.deepEqual(groups[0].filter, { group: 'fixture-group', statuses: ['normal'] })
    assert.equal(groups[0].config.intervalSeconds, 60)
    await bounds('group-list')
    await page.getByRole('button', { name: '暂停分组规则', exact: true }).click()
    await page.getByRole('button', { name: '恢复分组规则', exact: true }).waitFor()
    assert.equal(groups[0].config.enabled, false)
    await page.getByRole('button', { name: '恢复分组规则', exact: true }).click()
    await page.getByRole('button', { name: '暂停分组规则', exact: true }).waitFor()
    assert.equal(groups[0].config.enabled, true)
    await page.getByRole('button', { name: '删除分组规则', exact: true }).click()
    const groupDelete = page.getByRole('alertdialog', { name: '删除分组规则', exact: true })
    assert.equal(await groupDelete.getByRole('checkbox').isChecked(), false)
    await groupDelete.getByRole('button', { name: '确认', exact: true }).click()
    await groupDelete.waitFor({ state: 'hidden' })
    assert.equal(groupDeletes[0].deleteRules, false)
    assert.equal(rules.length, 2)
    await page.getByRole('button', { name: '账号监测', exact: true }).click()
    await page.getByRole('checkbox', { name: '全选搜索结果', exact: true }).locator('..').click()
    rejectRead = true
    await page.getByRole('button', { name: '批量删除', exact: true }).click()
    await page.getByRole('alert').filter({ hasText: 'fixture revision conflict' }).waitFor()
    assert.equal(deletes.length, 0)
    rejectRead = false
    rules[0].revision++
    await page.getByRole('button', { name: '批量删除', exact: true }).click()
    const confirmation = page.getByRole('alertdialog', { name: '批量删除检测规则', exact: true })
    await bounds('batch-delete', confirmation)
    rejectDelete = rules[1].id
    await confirmation.getByRole('button', { name: '确认', exact: true }).click()
    await confirmation.waitFor({ state: 'hidden' })
    await page.getByRole('alert').filter({ hasText: '1 项删除未确认' }).waitFor()
    assert.equal(deletes.length, 2)
    assert.equal(deletes[0].revision, 2)
    assert.equal(rules.length, 1)
    rejectDelete = ''
    await page.getByRole('button', { name: '批量删除', exact: true }).click()
    await confirmation.getByText(/已确认的 1 条/).waitFor()
    await confirmation.getByRole('button', { name: '确认', exact: true }).click()
    await confirmation.waitFor({ state: 'hidden' })
    assert.equal(deletes.length, 3)
    assert.equal(rules.length, 0)
    await page.goto(`http://127.0.0.1:${port}/accounts`)
    const row = page.locator(`tr[data-row-key="${accounts[0].id}"]`)
    await row.getByRole('button', { name: '更多操作', exact: true }).click()
    await page.getByRole('button', { name: '质量检测', exact: true }).click()
    await page.waitForURL(/quality-ops.*create=1/)
    const single = page.getByRole('dialog', { name: '新建检测规则', exact: true })
    await single.getByText('已选 1 项', { exact: true }).waitFor()
    assert.ok(page.url().includes(`accountId=${accounts[0].id}`))
    assert.deepEqual(errors, [])
    assert.deepEqual(external, [])
    process.stdout.write('PASS quality groups: create/filter/pause/resume/detach, fresh revisions, partial delete/retry, account menu and 1440/390/320px layouts; zero external requests\n')
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
