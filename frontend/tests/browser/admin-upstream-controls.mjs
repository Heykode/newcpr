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
  const origin = process.env.QA_BASE_URL || 'http://127.0.0.1:5398'
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-admin-controls-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ reducedMotion: 'reduce' })
  const errors = []
  const unexpected = []
  const passwordRequests = []
  const resets = []
  let failPassword = false
  let failReset = false
  page.on('pageerror', error => errors.push(error.message))
  const key = {
    id: 'key_synthetic',
    name: 'Synthetic administrative budget example',
    label: null,
    prefix: 'sk_example',
    enabled: true,
    maxConcurrency: 3,
    requestsPerMinute: 0,
    dailyLimitUsd: '10',
    weeklyLimitUsd: '70',
    dailyUsedUsd: '5',
    weeklyUsedUsd: '20',
    dailyResetsAt: '2026-10-01T00:00:00Z',
    weeklyResetsAt: '2026-10-07T00:00:00Z',
    createdAt: '2026-09-01T00:00:00Z',
    updatedAt: '2026-09-01T00:00:00Z',
    lastUsedAt: null,
    routingScope: 'all',
    groups: [],
    providerKinds: ['openai'],
  }
  const fulfill = (route, data) => route.fulfill({ json: { code: 200, message: 'ok', data } })
  await page.route('**/dev/api/**', async (route) => {
    const path = new URL(route.request().url()).pathname.replace(/^\/dev/, '')
    switch (path) {
      case '/api/admin/auth/status': return fulfill(route, { authenticated: true })
      case '/api/admin/system/version':
        return fulfill(route, { version: 'test', updateCached: true, hasUpdate: false })
      case '/api/admin/settings':
        return fulfill(route, {
          disableFast: false,
          refreshMarginSeconds: 1800,
          refreshConcurrency: 4,
          maxConcurrentPerAccount: 3,
          requestIntervalMs: 0,
          rotationStrategy: 'smart',
          usageRetentionDays: 31,
          opsEventRetentionDays: 30,
          auditRetentionDays: 90,
          modelMappings: {},
          requestTuning: {},
        })
      case '/api/admin/settings/admin-api-key': return fulfill(route, { exists: false })
      case '/api/admin/settings/openai-user-agent':
        return fulfill(route, {
          mode: 'default',
          defaultUserAgent: 'Synthetic/1.0',
          effectiveUserAgent: 'Synthetic/1.0',
          effectiveDesktopUserAgent: 'Synthetic/1.0',
          coreVersion: '1.0.0',
          desktopVersion: '1.0.0',
          osType: 'linux',
          osVersion: 'test',
          arch: 'x86_64',
          terminal: 'test',
          verified: true,
        })
      case '/api/admin/auth/password':
        passwordRequests.push(route.request().postDataJSON())
        if (failPassword)
          return route.fulfill({ status: 400, json: { code: 40001, message: '当前密码不正确', data: null } })
        return fulfill(route, { message: 'Password changed' })
      case '/api/admin/client-keys':
        return fulfill(route, { items: [key], total: 1, nextCursor: null })
      case '/api/admin/client-keys/reset-budget':
        resets.push(route.request().postDataJSON())
        if (failReset)
          return route.fulfill({ status: 503, json: { code: 50301, message: '预算暂不可用', data: null } })
        if (['daily', 'all'].includes(resets.at(-1).period))
          key.dailyUsedUsd = '0'
        if (['weekly', 'all'].includes(resets.at(-1).period))
          key.weeklyUsedUsd = '0'
        return fulfill(route, { id: key.id })
      default:
        unexpected.push(path)
        return route.fulfill({ status: 404, json: { code: 40400, data: null } })
    }
  })

  try {
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 900 })
      await page.goto(`${origin}/settings`)
      await page.getByLabel('当前密码').waitFor()
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
      const form = page.locator('form').filter({ has: page.getByLabel('当前密码') })
      assert.equal(await form.evaluate(element => element.scrollWidth <= element.clientWidth), true)
      await page.screenshot({ path: `${output}/password-${width}.png`, fullPage: true })
    }
    await page.getByLabel('当前密码').fill('synthetic-current-password')
    await page.getByLabel(/^新密码/).fill('synthetic-replacement-password')
    await page.getByLabel('确认新密码').fill('does-not-match')
    await page.getByRole('button', { name: '修改密码', exact: true }).click()
    await page.getByText('两次新密码不一致', { exact: true }).waitFor()
    assert.equal(passwordRequests.length, 0)
    failPassword = true
    await page.getByLabel('确认新密码').fill('synthetic-replacement-password')
    await page.getByRole('button', { name: '修改密码', exact: true }).click()
    await page.waitForFunction(() => [...document.querySelectorAll('input[type=password]')].every(input => input.value === ''))
    assert.equal(passwordRequests.length, 1)
    failPassword = false
    await page.getByLabel('当前密码').fill('synthetic-current-password')
    await page.getByLabel(/^新密码/).fill('synthetic-replacement-password')
    await page.getByLabel('确认新密码').fill('synthetic-replacement-password')
    await page.getByRole('button', { name: '修改密码', exact: true }).click()
    await page.waitForURL('**/login')
    assert.equal(passwordRequests.length, 2)

    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 900 })
      await page.goto(`${origin}/api-keys`)
      await page.getByRole('button', { name: '更多密钥操作', exact: true }).click()
      await page.getByRole('button', { name: '重置已用额度', exact: true }).click()
      const dialog = page.getByRole('alertdialog')
      await dialog.waitFor()
      assert.equal(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth), true)
      const box = await dialog.boundingBox()
      assert.ok(box.x >= 0 && box.x + box.width <= width)
      await page.screenshot({ path: `${output}/budget-${width}.png`, fullPage: true })
      await dialog.getByRole('button', { name: '取消', exact: true }).click()
      assert.equal(resets.length, 0)
    }
    await page.getByRole('button', { name: '更多密钥操作', exact: true }).click()
    await page.getByRole('button', { name: '重置已用额度', exact: true }).click()
    const dialog = page.getByRole('alertdialog')
    await dialog.getByRole('radio', { name: '日额度', exact: true }).click()
    failReset = true
    await dialog.getByRole('button', { name: '确认重置', exact: true }).click()
    await page.getByText('预算暂不可用', { exact: true }).waitFor()
    assert.equal(resets.length, 1)
    assert.equal(await dialog.isVisible(), true)
    assert.equal(key.dailyUsedUsd, '5')
    failReset = false
    await dialog.getByRole('button', { name: '确认重置', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.deepEqual(resets.at(-1), { id: 'key_synthetic', period: 'daily' })
    assert.equal(key.dailyUsedUsd, '0')
    assert.equal(key.weeklyUsedUsd, '20')
    assert.deepEqual(unexpected, [])
    assert.deepEqual(errors, [])
    process.stdout.write('Passed: password validation, failure clearing, relogin; compact Key menu, reset selection/cancel/error/retry; 1440/390/320px; synthetic API only.\n')
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
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
