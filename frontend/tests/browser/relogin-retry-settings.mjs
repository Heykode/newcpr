/* eslint-disable no-console -- standalone browser regression runner. */
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-relogin-retry-settings-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
  const errors = []
  const mutations = []
  let settings = { concurrency: 1, paused: false, maxRetries: 2, retryIntervalMinutes: 5 }
  let failSave = false
  let reads = 0
  const now = new Date().toISOString()
  const rows = ['cooldown', 'manual_required', 'uncertain'].map(state => ({
    id: state,
    revision: 1,
    email: `${state}@example.invalid`,
    hasTotp: true,
    automatic: true,
    status: state === 'uncertain' ? 'uncertain' : 'failed',
    message: '',
    planType: 'team',
    workspaceId: 'workspace-fixture',
    preferredWorkspaceId: null,
    credentialStatus: 'none',
    poolStatus: 'present',
    poolAccountIds: [`account-${state}`],
    poolAccounts: [],
    reloginAccountId: `account-${state}`,
    reloginCount: 0,
    lastReloginAt: null,
    verifiedAt: null,
    expiresAt: null,
    updatedAt: now,
    importedAt: now,
    recovery: {
      state,
      message: state === 'manual_required'
        ? '原工作区不可访问'
        : '等待处理',
      retryAt: state === 'cooldown' ? new Date(Date.now() + 300000).toISOString() : null,
      retriesUsed: 1,
      maxRetries: 2,
    },
  }))
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/dev/api/**', async (route) => {
    const request = route.request()
    const path = new URL(request.url()).pathname.replace('/dev', '')
    let data = null
    if (path.endsWith('/auth/status')) {
      data = { authenticated: true }
    }
    else if (path === '/api/admin/relogin') {
      reads++
      data = { items: rows, settings }
    }
    else if (path === '/api/admin/relogin/settings') {
      const body = request.postDataJSON()
      mutations.push(body)
      if (failSave) {
        await route.fulfill({ status: 503, json: { code: 503, message: '模拟保存失败', data: null } })
        return
      }
      settings = { ...settings, ...body }
    }
    else if (path.endsWith('/system/version')) {
      data = { version: 'test', buildType: 'test' }
    }
    else if (request.method() !== 'GET') {
      throw new Error(`Unexpected mutation: ${path}`)
    }
    await route.fulfill({ json: { code: 200, message: 'ok', data } })
  })
  const open = () => page.getByRole('button', { name: '自动重登设置', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: '自动重登设置', exact: true })
  const retries = dialog.getByRole('spinbutton', { name: '失败重试次数', exact: true })
  const interval = dialog.getByRole('spinbutton', { name: '重试间隔（分钟）', exact: true })
  const save = () => dialog.getByRole('button', { name: '保存', exact: true }).click()
  const cancel = () => dialog.getByRole('button', { name: '取消', exact: true }).click()
  try {
    await page.goto(`${process.env.QA_BASE_URL || 'http://127.0.0.1:5242'}/relogin`)
    await page.getByText('cooldown@example.invalid', { exact: true }).waitFor()
    const stopped = page.locator('tr[data-row-key="manual_required"]')
    const uncertain = page.locator('tr[data-row-key="uncertain"]')
    await stopped.getByText('需人工处理', { exact: true }).waitFor()
    await stopped.getByText('原工作区不可访问', { exact: true }).waitFor()
    await uncertain.getByText('推送待核实', { exact: true }).waitFor()
    assert.equal(await uncertain.getByText('已重试', { exact: false }).count(), 0)
    assert.ok(await uncertain.getByRole('button', { name: '推送', exact: true }).isDisabled())
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 900 })
      const cell = stopped.locator('[data-column-key="status"]')
      await cell.scrollIntoViewIfNeeded()
      assert.ok(await cell.evaluate(node => [...node.children].every(child => child.scrollWidth <= child.clientWidth + 1)))
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1))
      await page.screenshot({ path: `${output}/status-${width}.png`, fullPage: true })
    }
    await page.setViewportSize({ width: 1440, height: 1000 })
    await open()
    assert.equal(await retries.inputValue(), '2')
    assert.equal(await interval.inputValue(), '5')
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 900 })
      const box = await dialog.boundingBox()
      assert.ok(box.x >= 0 && box.x + box.width <= width + 1)
      assert.ok(await dialog.evaluate(node => [...node.querySelectorAll('label, input, button')].every(child => child.scrollWidth <= child.clientWidth + 1)))
      await page.screenshot({ path: `${output}/settings-${width}.png`, fullPage: true })
    }
    await page.setViewportSize({ width: 1440, height: 1000 })
    await retries.fill('0')
    await interval.fill('23')
    await save()
    await dialog.waitFor({ state: 'hidden' })
    assert.deepEqual(mutations.at(-1), { concurrency: 1, paused: false, maxRetries: 0, retryIntervalMinutes: 23 })
    await open()
    assert.equal(await retries.inputValue(), '0')
    assert.equal(await interval.inputValue(), '23')
    const count = mutations.length
    for (const value of ['', '-1', '11', '1.5']) {
      await retries.fill(value)
      await save()
      await dialog.getByRole('alert').filter({ hasText: '失败重试次数必须为 0 至 10' }).waitFor()
      assert.equal(mutations.length, count)
    }
    await retries.fill('0')
    for (const value of ['', '0', '1441', '1.5']) {
      await interval.fill(value)
      await save()
      await dialog.getByRole('alert').filter({ hasText: '重试间隔必须为 1 至 1440 分钟' }).waitFor()
      assert.equal(mutations.length, count)
    }
    await retries.fill('7')
    await interval.fill('31')
    const oldReads = reads
    await page.waitForResponse(response => new URL(response.url()).pathname === '/dev/api/admin/relogin')
    assert.ok(reads > oldReads)
    assert.equal(await retries.inputValue(), '7')
    assert.equal(await interval.inputValue(), '31')
    await cancel()
    await open()
    assert.equal(await retries.inputValue(), '0')
    assert.equal(await interval.inputValue(), '23')
    failSave = true
    await retries.fill('3')
    await save()
    await dialog.getByRole('alert').filter({ hasText: '模拟保存失败' }).waitFor()
    assert.equal(await retries.inputValue(), '3')
    assert.equal(settings.maxRetries, 0)
    failSave = false
    await save()
    await dialog.waitFor({ state: 'hidden' })
    assert.equal(settings.maxRetries, 3)
    await page.getByRole('spinbutton', { name: '重登并发', exact: true }).fill('2')
    await page.getByRole('button', { name: '保存并发设置', exact: true }).click()
    await page.getByRole('button', { name: '暂停重登队列', exact: true }).click()
    await page.getByRole('button', { name: '恢复重登队列', exact: true }).waitFor()
    assert.deepEqual(mutations.at(-2), { concurrency: 2, paused: false })
    assert.deepEqual(mutations.at(-1), { concurrency: 2, paused: true })
    assert.equal(settings.maxRetries, 3)
    assert.equal(settings.retryIntervalMinutes, 23)
    await page.getByRole('button', { name: '切换暗黑模式', exact: true }).click()
    await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark')
    await open()
    await page.screenshot({ path: `${output}/settings-dark.png`, fullPage: true })
    await cancel()
    await page.getByRole('combobox', { name: '处理状态筛选' }).click()
    await page.getByRole('option', { name: '需人工处理', exact: true }).click()
    assert.equal(await page.locator('tbody tr').count(), 1)
    assert.deepEqual(errors, [])
    console.log(JSON.stringify({ result: 'passed', assertions: 'zero, persistence, validation, cancel, polling, failed save, legacy updates, terminal/uncertain status, desktop/mobile', output }))
  }
  catch (error) {
    await page.screenshot({ path: `${output}/failure.png`, fullPage: true })
    throw error
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
