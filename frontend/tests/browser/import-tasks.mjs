import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

// Run against an already-running local frontend. All API traffic is synthetic.
async function main() {
  const base = new URL(process.env.QA_BASE_URL || 'http://127.0.0.1:5179')
  assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname), 'QA_BASE_URL must be loopback')
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-import-tasks-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({
    viewport: { width: 1440, height: 1000 },
    reducedMotion: 'reduce',
    serviceWorkers: 'block',
  })
  const errors = []
  const assetFailures = []
  const unexpected = []
  const submissions = []
  const stops = []
  const jobs = new Map()
  let loseFirstResponse = true
  let failReads = false
  let accountReads = 0
  const emptyPage = { items: [], page: { page: 1, pageSize: 20, total: 0, totalPages: 1 }, configRevision: 1 }
  page.on('pageerror', error => errors.push(error.message))
  page.on('response', (response) => {
    const path = new URL(response.url()).pathname
    if (response.status() >= 400 && !/^\/(?:dev\/)?api\//.test(path))
      assetFailures.push(`${response.status()} ${path}`)
  })

  function summarize(job) {
    const counts = { pending: 0, running: 0, succeeded: 0, failed: 0, unknown: 0, skipped: 0, importedAccounts: 0 }
    for (const item of job.items) {
      counts[item.status]++
      counts.importedAccounts += item.accountIds.length
    }
    return { ...job, counts, total: job.items.length }
  }

  await page.route('**/*', async (route) => {
    const request = route.request()
    const url = new URL(request.url())
    if (url.origin !== base.origin) {
      unexpected.push(`external ${url.origin}`)
      return route.abort()
    }
    const path = url.pathname.replace(/^\/dev(?=\/api\/)/, '')
    if (!path.startsWith('/api/'))
      return route.continue()
    let data
    if (path === '/api/admin/accounts/import-tasks' && request.method() === 'POST') {
      const body = request.postDataJSON()
      submissions.push(body)
      if (!jobs.has(body.submissionId)) {
        jobs.set(body.submissionId, {
          taskId: body.submissionId,
          createdAt: new Date().toISOString(),
          finishedAt: null,
          stopRequested: false,
          items: body.items.map((item, index) => ({
            index: index + 1,
            provider: item.provider,
            status: index === 0 ? 'running' : 'pending',
            accountIds: [],
            message: null,
          })),
        })
      }
      data = summarize(jobs.get(body.submissionId))
      if (loseFirstResponse) {
        loseFirstResponse = false
        return route.abort('failed')
      }
    }
    else if (path === '/api/admin/accounts/import-tasks/stop' && request.method() === 'POST') {
      const body = request.postDataJSON()
      stops.push(body)
      const job = jobs.get(body.taskId)
      assert.ok(job)
      job.stopRequested = true
      for (const item of job.items) {
        if (item.status === 'pending')
          item.status = 'skipped'
      }
      data = summarize(job)
    }
    else if (request.method() !== 'GET') {
      unexpected.push(`${request.method()} ${path}`)
      return route.fulfill({ status: 405, json: { message: 'Unexpected synthetic mutation' } })
    }
    else if (path === '/api/admin/accounts/import-tasks' || path === '/api/admin/accounts/import-tasks/detail') {
      if (failReads)
        return route.fulfill({ status: 503, json: { message: 'Synthetic progress read failure' } })
      data = path.endsWith('/detail')
        ? summarize(jobs.get(url.searchParams.get('taskId')))
        : { items: [...jobs.values()].reverse().map(summarize) }
    }
    else if (path === '/api/admin/auth/status') {
      data = { authenticated: true }
    }
    else if (path === '/api/admin/accounts') {
      accountReads++
      data = { ...emptyPage, summary: { total: 0, normal: 0, error: 0, disabled: 0, quotaExhausted: 0, rateLimited: 0 } }
    }
    else if (path === '/api/admin/account-groups' || path === '/api/admin/proxies') {
      data = emptyPage
    }
    else if (path === '/api/admin/relogin') {
      data = { ...emptyPage, items: [] }
    }
    else if (path === '/api/admin/system/version') {
      data = { version: 'test', hasUpdate: false, deploymentMode: 'test', updateWarning: null }
    }
    else {
      unexpected.push(`GET ${path}`)
      return route.fulfill({ status: 404, json: { message: 'Unexpected synthetic read' } })
    }
    return route.fulfill({ json: { code: 200, message: 'ok', data } })
  })

  async function openTasks() {
    await page.getByRole('button', { name: /^导入任务/ }).click()
    await page.getByRole('heading', { name: '导入任务', exact: true }).waitFor()
  }

  async function closeTasks() {
    const dialog = page.getByRole('dialog', { name: '导入任务', exact: true })
    await dialog.getByRole('contentinfo').getByRole('button', { name: '关闭', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
  }

  async function screenshots(label, dialogName = '导入任务') {
    await page.getByRole('button', { name: /^关闭.*通知$/ }).first().waitFor({ state: 'hidden' })
    for (const theme of ['light', 'dark']) {
      await page.emulateMedia({ colorScheme: theme })
      await page.waitForFunction(theme => document.documentElement.dataset.theme === theme, theme)
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: width < 500 ? 844 : 1000 })
        const dialog = page.getByRole('dialog', { name: dialogName, exact: true })
        await dialog.waitFor()
        const bounds = await dialog.boundingBox()
        assert.ok(bounds && bounds.x >= -1 && bounds.x + bounds.width <= width + 1, `${label} ${width}: dialog fits`)
        assert.ok(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1), `${label} ${width}: no dialog overflow`)
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1), `${label} ${width}: no document overflow`)
        await page.screenshot({ path: `${output}/${label}-${theme}-${width}.png`, fullPage: true })
      }
    }
    await page.setViewportSize({ width: 1440, height: 1000 })
  }

  try {
    await page.goto(new URL('/accounts', base).href)
    await page.getByRole('heading', { name: '账号管理', exact: true }).waitFor()
    await openTasks()
    await page.getByText('暂无导入任务', { exact: true }).waitFor()
    await closeTasks()

    await page.getByRole('button', { name: '导入账号', exact: true }).click()
    await page.getByRole('group', { name: '选择账号平台' }).getByRole('button', { name: 'OpenAI', exact: true }).click()
    const stateSwitch = page.getByRole('switch', { name: '切换 Turn State 注入' })
    assert.equal(await stateSwitch.isChecked(), false)
    await stateSwitch.locator('..').click()
    assert.equal(await stateSwitch.isChecked(), true)
    await page.getByRole('group', { name: '选择账号平台' }).getByRole('button', { name: 'xAI', exact: true }).click()
    assert.equal(await stateSwitch.count(), 0, 'State is unavailable for xAI')
    await page.getByRole('group', { name: '选择账号平台' }).getByRole('button', { name: '批量导入', exact: true }).click()
    assert.equal(await stateSwitch.isChecked(), true)
    await page.getByRole('group', { name: '选择账号平台' }).getByRole('button', { name: 'OpenAI', exact: true }).click()
    await screenshots('import-state-settings', '账号设置')
    await page.getByRole('button', { name: '继续导入', exact: true }).click()
    await page.getByRole('radio', { name: 'RT', exact: true }).click()
    await page.getByRole('textbox', { name: 'Refresh Token', exact: true }).fill('synthetic-first\nsynthetic-second')
    const submit = page.getByRole('button', { name: '创建导入任务', exact: true })
    await submit.evaluate((button) => {
      button.click()
      button.click()
    })
    await page.waitForFunction(() => {
      const button = [...document.querySelectorAll('button')].find(button => button.textContent.includes('创建导入任务'))
      return button && !button.disabled
    })
    assert.equal(submissions.length, 1, 'double click must not create a second POST')
    await submit.click()
    await page.getByRole('heading', { name: '导入任务', exact: true }).waitFor()
    await page.getByRole('dialog', { name: '导入账号', exact: true }).waitFor({ state: 'hidden' })
    await page.getByRole('progressbar').waitFor()
    assert.equal(submissions.length, 2)
    assert.equal(submissions[0].submissionId, submissions[1].submissionId, 'lost response retry uses the same server ID')
    assert.equal(jobs.size, 1)
    assert.deepEqual(submissions[0].items.map(item => item.data), [
      { accounts: [{ refreshToken: 'synthetic-first' }] },
      { accounts: [{ refreshToken: 'synthetic-second' }] },
    ])
    assert.ok(submissions[0].items.every(item => item.settings.turnStateInjectionEnabled === true))
    await screenshots('running')

    await closeTasks()
    await page.reload()
    await openTasks()
    await page.getByRole('button', { name: '停止未开始条目', exact: true }).waitFor()
    assert.equal(submissions.length, 2, 'reopening recovers server tasks without posting credentials')
    failReads = true
    await page.getByRole('alert').filter({ hasText: '进度暂未更新' }).waitFor({ timeout: 10000 })
    failReads = false
    await page.getByRole('button', { name: '刷新进度', exact: true }).click()
    await page.getByRole('alert').filter({ hasText: '进度暂未更新' }).waitFor({ state: 'hidden' })
    await page.getByRole('button', { name: '停止未开始条目', exact: true }).click()
    await page.getByRole('button', { name: '等待当前条目结束', exact: true }).waitFor()
    assert.equal(stops.length, 1)
    const job = [...jobs.values()][0]
    assert.deepEqual(job.items.map(item => item.status), ['running', 'skipped'])
    assert.equal(await page.getByRole('button', { name: '等待当前条目结束', exact: true }).isDisabled(), true)
    await screenshots('stopping')

    job.items[0].status = 'unknown'
    job.items[0].message = `Synthetic indeterminate result <script> ${'diagnostic-detail-'.repeat(18)}`
    job.finishedAt = new Date().toISOString()
    await page.getByText(/不要直接重试原 Token/).waitFor()
    await screenshots('unknown')
    assert.equal(submissions.length, 2)
    assert.equal(await page.getByRole('button', { name: '停止未开始条目', exact: true }).count(), 0)
    await page.getByRole('button', { name: '返回账号列表', exact: true }).click()
    await page.getByRole('dialog').waitFor({ state: 'hidden' })
    assert.ok(accountReads >= 2)

    await page.reload()
    await openTasks()
    await page.getByText(/不要直接重试原 Token/).waitFor()
    assert.equal(submissions.length, 2)

    await closeTasks()
    await page.getByRole('button', { name: '导入账号', exact: true }).click()
    await page.getByRole('group', { name: '选择账号平台' }).getByRole('button', { name: 'OpenAI', exact: true }).click()
    assert.equal(await stateSwitch.isChecked(), false, 'a new form resets the switch')
    await page.getByRole('button', { name: '继续导入', exact: true }).click()
    await page.getByRole('radio', { name: 'RT', exact: true }).click()
    await page.getByRole('textbox', { name: 'Refresh Token', exact: true }).fill('synthetic-third\nsynthetic-fourth\nsynthetic-fifth')
    await page.getByRole('button', { name: '创建导入任务', exact: true }).click()
    await page.getByRole('heading', { name: '导入任务', exact: true }).waitFor()
    assert.equal(jobs.size, 2)
    assert.equal(submissions[2].items[0].settings.turnStateInjectionEnabled, false)
    const batches = page.getByRole('combobox', { name: '切换导入任务' })
    await batches.click()
    await page.getByRole('option').filter({ hasText: '2 个条目' }).click()
    await page.getByText(/不要直接重试原 Token/).waitFor()
    await batches.click()
    await page.getByRole('option').filter({ hasText: '3 个条目' }).click()
    await page.getByRole('heading', { name: '导入中', exact: true }).waitFor()
    const second = [...jobs.values()][1]
    second.items[0].status = 'succeeded'
    second.items[0].accountIds = ['acct_saved_example', 'acct_saved_example']
    const savedEmail = `${'long-imported-account-'.repeat(3)}+workspace@example.com`
    second.items[0].accountEmails = { acct_saved_example: savedEmail }
    second.items[1].status = 'failed'
    second.items[1].message = 'Synthetic rejected credential'
    second.items[2].status = 'unknown'
    second.items[2].message = 'Synthetic indeterminate import'
    second.finishedAt = new Date().toISOString()
    await page.getByRole('heading', { name: '已结束 · 有待处理项', exact: true }).waitFor()
    await page.getByText('已导入账号（2）', { exact: true }).click()
    assert.equal(await page.getByText(savedEmail, { exact: true }).count(), 2)
    assert.equal(await page.getByText('acct_saved_example', { exact: true }).count(), 0)
    await page.getByText('Synthetic rejected credential', { exact: true }).waitFor()
    await page.getByRole('region', { name: '导入任务详情' }).locator('p').filter({ hasText: /结束/ }).waitFor()
    await page.getByRole('radio', { name: '需处理', exact: true }).click()
    await page.getByText('已导入账号（2）', { exact: true }).waitFor({ state: 'hidden' })
    await page.getByText('Synthetic rejected credential', { exact: true }).waitFor()
    await page.getByRole('radio', { name: '全部', exact: true }).click()
    await page.getByText('已导入账号（2）', { exact: true }).click()
    assert.equal(await page.getByText(savedEmail, { exact: true }).count(), 2)
    await screenshots('multiple-batches')

    await page.reload()
    await openTasks()
    await batches.click()
    assert.equal(await page.getByRole('option').count(), 2, 'reload retains both server batches')
    await page.getByRole('option').filter({ hasText: '2 个条目' }).click()
    await page.getByText(/不要直接重试原 Token/).waitFor()
    assert.equal(submissions.length, 3)

    // Retention/restart removes task metadata, not the already-imported accounts.
    jobs.clear()
    await page.getByText('暂无导入任务', { exact: true }).waitFor()
    await page.getByText(/完成后保留 1 小时/).waitFor()
    assert.equal(submissions.length, 3, 'lost history must not replay credentials')
    assert.deepEqual(errors, [])
    assert.deepEqual(assetFailures, [])
    assert.deepEqual(unexpected, [])
    process.stdout.write(`Passed synthetic import lifecycle; screenshots: ${output}\n`)
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
