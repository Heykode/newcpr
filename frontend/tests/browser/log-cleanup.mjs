import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { isolateNetwork } from './usage-columns.mjs'

async function main() {
  const base = new URL(process.env.QA_BASE_URL || 'http://127.0.0.1:5289')
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-log-cleanup-ui'
  const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href)
  const browser = await chromium.launch({ channel: 'chrome', headless: true })
  await mkdir(output, { recursive: true })
  const report = { blocked: [], api: [], errors: [], widths: [] }
  try {
    for (const width of [1440, 390, 320]) {
      const context = await browser.newContext({ viewport: { width, height: 1050 }, serviceWorkers: 'block' })
      await isolateNetwork(context, base, report)
      let state = {
        revision: 1,
        nextRunAt: null,
        job: null,
        config: {
          enabled: false,
          frequency: 'daily',
          dailyHour: 3,
          dailyMinute: 0,
          timezone: 'Asia/Shanghai',
          requests: { selected: true, retentionDays: 31 },
          files: { selected: true, retentionDays: 7 },
          captures: { selected: false, retentionDays: 7 },
          audit: { selected: false, retentionDays: 90 },
        },
      }
      let finished = false
      let usageCalls = 0
      let rejectNextSave = false
      await context.route('**/*', async (route) => {
        const url = new URL(route.request().url())
        const path = url.pathname.replace(/^\/dev(?=\/api\/)/, '')
        if (url.origin === base.origin && route.request().method() === 'GET' && path.startsWith('/tests/fixtures/'))
          return route.continue()
        if (url.origin !== base.origin || !path.startsWith('/api/'))
          return route.fallback()
        let data
        if (path === '/api/admin/log-cleanup' && route.request().method() === 'POST') {
          if (rejectNextSave) {
            rejectNextSave = false
            return route.fulfill({ status: 503, contentType: 'application/json', body: JSON.stringify({ code: 503, message: '依赖不可用', data: null }) })
          }
          state = { ...state, ...route.request().postDataJSON(), revision: state.revision + 1 }
          data = null
        }
        else if (path === '/api/admin/log-cleanup') {
          if (finished && state.job)
            state.job = { ...state.job, status: 'succeeded', categoryIndex: 4, removed: 123 }
          data = state
        }
        else if (path === '/api/admin/log-cleanup/usage') {
          usageCalls++
          data = { measuredAt: '2026-09-30T00:00:00Z', items: [
            { category: 'requests', bytes: 27320000000 },
            { category: 'files', bytes: finished ? 1000000000 : 6700000000 },
            { category: 'captures', bytes: 0 },
            { category: 'audit', bytes: null },
          ] }
        }
        else if (path === '/api/admin/log-cleanup/preview') {
          data = { revision: state.revision, config: state.config, cutoffAt: '2026-09-30T00:00:00Z' }
        }
        else if (path === '/api/admin/log-cleanup/start') {
          state.job = { id: 'cleanup-fixture', status: 'running', categoryIndex: 0, removed: 0, errors: [] }
          data = state.job
        }
        else {
          return route.abort()
        }
        return route.fulfill({ contentType: 'application/json', body: JSON.stringify({ code: 200, message: 'ok', data }) })
      })
      const page = await context.newPage()
      page.on('pageerror', e => report.errors.push(e.message))
      await page.goto(new URL('/tests/fixtures/log-cleanup.html', base).href)
      await page.getByText('27.32 GB', { exact: true }).waitFor()
      assert.equal(await page.getByText('读取失败', { exact: true }).count(), 1)
      assert.equal(await page.getByText('0 B', { exact: true }).count(), 1)
      assert.equal(await page.getByText('上次清理结果', { exact: true }).count(), 0)
      assert(await page.getByRole('button', { name: '立即清理', exact: true }).isEnabled())
      const requestDays = page.getByRole('spinbutton', { name: '请求日志（含索引）保留天数', exact: true })
      const requestGroup = page.getByRole('group', { name: '请求日志（含索引）保留设置', exact: true })
      const allRequests = requestGroup.getByRole('checkbox', { name: '全部清理', exact: true })
      const allRequestsLabel = requestGroup.getByText('全部清理', { exact: true })
      await requestDays.fill('3')
      await requestDays.blur()
      rejectNextSave = true
      await page.getByRole('button', { name: '保存设置', exact: true }).click()
      await page.getByRole('alert').filter({ hasText: '依赖不可用' }).waitFor()
      assert.equal(state.config.requests.retentionDays, 31, 'failed save keeps persisted settings')
      assert.equal(await requestDays.inputValue(), '3', 'failed save keeps the editable draft')
      await page.getByRole('button', { name: '保存设置', exact: true }).click()
      await page.getByText('设置尚未保存', { exact: true }).waitFor({ state: 'hidden' })
      await page.reload()
      await requestDays.waitFor()
      assert.equal(await requestDays.inputValue(), '3', 'saved three-day retention reloads')
      await allRequestsLabel.click()
      assert(await requestDays.isDisabled())
      await allRequestsLabel.click()
      assert.equal(await requestDays.inputValue(), '3', 'unchecking restores the positive draft')
      await allRequestsLabel.click()
      await page.getByRole('button', { name: '保存设置', exact: true }).click()
      await page.getByText('设置尚未保存', { exact: true }).waitFor({ state: 'hidden' })
      assert.equal(state.config.requests.retentionDays, 0, 'clear-all retains the existing API encoding')
      await page.reload()
      await allRequests.waitFor()
      assert(await allRequests.isChecked(), 'legacy zero reloads as explicit clear-all')
      assert(await requestDays.isDisabled())
      assert.equal(await requestDays.getAttribute('min'), '1')
      await allRequestsLabel.click()
      assert.equal(await requestDays.inputValue(), '1', 'legacy zero returns to a positive default')
      await requestDays.fill('4')
      await requestDays.blur()
      await allRequestsLabel.click()
      for (const label of ['运行日志', '错误采集记录', '管理员操作日志']) {
        const group = page.getByRole('group', { name: `${label}保留设置`, exact: true })
        const checkbox = group.getByRole('checkbox', { name: '全部清理', exact: true })
        const input = group.getByRole('spinbutton')
        const previous = await input.inputValue()
        await group.getByText('全部清理', { exact: true }).click()
        assert(await checkbox.isChecked())
        assert(await input.isDisabled(), `${label} clear-all disables numeric input`)
        await group.getByText('全部清理', { exact: true }).click()
        assert(!await checkbox.isChecked())
        assert.equal(await input.inputValue(), previous, `${label} restores its own retention`)
      }
      const days = page.getByRole('spinbutton', { name: '运行日志保留天数', exact: true })
      await days.fill('0')
      await days.blur()
      assert.equal(await days.inputValue(), '1', 'typing zero must not enable clear-all')
      await days.fill('4')
      await days.blur()
      assert(await page.getByRole('button', { name: '立即清理', exact: true }).isDisabled())
      await page.getByRole('button', { name: '保存设置', exact: true }).click()
      await page.getByRole('button', { name: '立即清理', exact: true }).click()
      await page.getByRole('button', { name: '确认清理', exact: true }).waitFor()
      assert(await page.getByText(/请求日志（含索引）：全部清理已结束历史/).isVisible())
      assert(await page.getByText(/运行日志：保留最近4天/).isVisible())
      assert(await page.getByText('删除请求日志会一并删除诊断轨迹和关联运维事件，历史明细统计也将减少。', { exact: true }).isVisible())
      await page.screenshot({ path: `${output}/confirm-${width}.png`, fullPage: true, animations: 'disabled' })
      await page.getByRole('button', { name: '确认清理', exact: true }).click()
      await page.getByRole('button', { name: '停止清理', exact: true }).waitFor()
      assert(await page.getByRole('button', { name: '立即清理', exact: true }).isDisabled())
      const beforeCompletionUsage = usageCalls
      finished = true
      await page.getByText('1.00 GB', { exact: true }).waitFor({ timeout: 15000 })
      assert.equal(usageCalls, beforeCompletionUsage + 1, 'completion refreshes current occupancy')
      assert.equal(await page.getByRole('button', { name: '停止清理', exact: true }).count(), 0)
      assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `overflow at ${width}`)
      await page.screenshot({ path: `${output}/cleanup-${width}.png`, fullPage: true, animations: 'disabled' })
      report.widths.push(width)
      await context.close()
    }
    assert.deepEqual(report.errors, [])
    process.stdout.write(`${JSON.stringify(report)}\n`)
  }
  finally {
    await browser.close()
  }
}
main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
