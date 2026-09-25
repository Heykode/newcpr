import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const port = '5192'
  const server = spawn(process.execPath, ['tests/relogin-count-preview.mjs'], {
    env: { ...process.env, QA_PORT: port },
    stdio: 'ignore',
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-excel-operations-ui'
  await mkdir(output, { recursive: true })
  let browser
  try {
    for (let i = 0; i < 100; i++) {
      try {
        if ((await fetch(`http://127.0.0.1:${port}/token-guard`)).ok)
          break
      }
      catch {}
      await new Promise(resolve => setTimeout(resolve, 100))
    }
    browser = await chromium.launch({ headless: true })
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, reducedMotion: 'reduce' })
    const errors = []
    page.on('pageerror', error => errors.push(error.message))
    const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
    const guard = {
      config: { enabled: false, groupIds: [], model: 'gpt-6-astra', intervalSeconds: 300, timeoutSeconds: 60, concurrency: 6, maxPerCycle: 12 },
      running: false,
      queued: false,
      events: [
        { accountId: 'fixture-account-with-a-long-identifier', observedRevision: 1, outcome: 'auth_required', reason: 'credential_expired', latencyMs: 200, observedAt: '2026-01-01T00:00:00Z' },
      ],
    }
    const capture = {
      config: { enabled: false, quotaMib: 1024, retentionDays: 7 },
      instanceId: 'fixture-instance',
      tasks: [],
      records: [],
      skipped: 0,
      activeSessions: 0,
      bufferedBytes: 0,
      storageFault: false,
    }
    let runs = 0
    let logins = 0
    let creates = 0
    await page.route('**/dev/api/admin/accounts?**', route => fulfill(route, {
      items: [{ id: 'fixture-account-with-a-long-identifier', name: 'Capture fixture' }],
      page: { page: 1, pageSize: 30, total: 1, totalPages: 1 },
    }))
    await page.route('**/dev/api/admin/token-guard**', (route) => {
      const pathname = new URL(route.request().url()).pathname
      if (pathname.endsWith('/config')) {
        guard.config = route.request().postDataJSON()
        return fulfill(route, null)
      }
      if (pathname.endsWith('/run')) {
        runs++
        guard.queued = true
        return fulfill(route, null)
      }
      if (pathname.endsWith('/relogin')) {
        logins++
        return fulfill(route, null)
      }
      return fulfill(route, guard)
    })
    await page.route('**/dev/api/admin/request-captures**', (route) => {
      const pathname = new URL(route.request().url()).pathname
      if (pathname.endsWith('/config')) {
        capture.config = route.request().postDataJSON()
        return fulfill(route, null)
      }
      if (pathname.includes('/records/')) {
        return fulfill(route, { text: '{"input":"<script>window.__captureExecuted=true</script>"}', nextOffset: null })
      }
      if (route.request().method() === 'DELETE') {
        capture.tasks = []
        capture.records = []
        return fulfill(route, null)
      }
      if (pathname.endsWith('/stop')) {
        capture.tasks[0].status = 'stopped'
        return fulfill(route, null)
      }
      if (route.request().method() === 'POST') {
        creates++
        const input = route.request().postDataJSON()
        assert.equal(input.minutes, 15)
        assert.equal(input.includeMedia, false)
        const task = { ...input, id: 'fixture-task', status: 'running', startedAt: '2026-01-01T00:00:00Z', expiresAt: '2026-01-01T00:15:00Z' }
        capture.tasks.push(task)
        capture.records.push({ id: 'fixture-record', taskId: task.id, requestId: 'req_fixture_very_long_request_identifier', bytes: 100, incomplete: true, createdAt: task.startedAt })
        return fulfill(route, task)
      }
      return fulfill(route, capture)
    })
    await page.goto(`http://127.0.0.1:${port}/token-guard`)
    await page.getByRole('heading', { name: '凭证守护', exact: true }).waitFor()
    await page.getByRole('switch', { name: /凭证守护/ }).waitFor()
    assert.equal(await page.getByRole('button', { name: '立即巡检', exact: true }).isDisabled(), true)
    await page.getByRole('switch', { name: /凭证守护/ }).locator('..').click()
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await page.getByRole('button', { name: '立即巡检', exact: true }).click()
    assert.equal(runs, 1)
    await page.getByRole('button', { name: '重新登录', exact: true }).click()
    assert.equal(logins, 1)
    for (const path of ['token-guard', 'request-captures']) {
      await page.goto(`http://127.0.0.1:${port}/${path}`)
      const title = path === 'token-guard' ? '凭证守护' : '请求采集'
      await page.getByRole('heading', { name: title, exact: true }).waitFor()
      if (path === 'request-captures') {
        await page.getByRole('switch', { name: /启用错误采集/ }).locator('..').click()
        await page.getByRole('button', { name: '保存', exact: true }).click()
        await page.getByRole('combobox', { name: '选择采集目标', exact: true }).click()
        await page.getByRole('option', { name: /Capture fixture/ }).click()
        await page.getByRole('button', { name: '开始采集', exact: true }).click()
        await page.getByRole('button', { name: '查看正文', exact: true }).click()
        await page.locator('pre').filter({ hasText: '<script>' }).waitFor()
        assert.equal(await page.evaluate(() => window.__captureExecuted), undefined)
        assert.equal(creates, 1)
      }
      for (const theme of ['light', 'dark']) {
        await page.setViewportSize({ width: 1440, height: 900 })
        if (await page.locator('html').getAttribute('data-theme') !== theme)
          await page.getByRole('button', { name: theme === 'dark' ? '切换暗黑模式' : '切换浅色模式', exact: true }).click()
        for (const width of [1440, 390, 320]) {
          await page.setViewportSize({ width, height: 900 })
          assert.ok(await page.locator('main').evaluate(element => element.scrollWidth <= element.clientWidth + 1))
          assert.ok(await page.locator('form').last().evaluate((form) => {
            const controls = [...form.querySelectorAll('input, button[role="combobox"]')]
              .map(element => element.getBoundingClientRect())
              .filter(rect => rect.width > 0)
            return controls.every((rect, i) => controls.slice(i + 1).every(other =>
              rect.right <= other.left + 1 || other.right <= rect.left + 1
              || rect.bottom <= other.top + 1 || other.bottom <= rect.top + 1))
          }), `${path} form controls overlap at ${width}`)
          await page.screenshot({ path: `${output}/${path}-${theme}-${width}.png`, fullPage: true })
        }
      }
    }
    await page.setViewportSize({ width: 1440, height: 900 })
    await page.getByRole('button', { name: '停止采集', exact: true }).click()
    await page.getByText('已停止', { exact: true }).waitFor()
    assert.match(await page.getByRole('link', { name: '导出记录' }).getAttribute('href'), /^\/dev\/api\/admin\/request-captures\/records\//)
    assert.match(await page.getByRole('link', { name: '导出任务' }).getAttribute('href'), /\/fixture-task\/export$/)
    await page.getByRole('button', { name: '删除任务', exact: true }).click()
    const dialog = page.getByRole('alertdialog')
    await dialog.waitFor()
    await dialog.getByRole('button', { name: '确认', exact: true }).click()
    await page.getByText('暂无错误记录', { exact: true }).waitFor()
    assert.equal(await page.locator('pre').count(), 0)
    assert.deepEqual(errors, [])
    process.stdout.write('Excel operations browser checks passed\n')
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
