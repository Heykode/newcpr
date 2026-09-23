import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { groups, monitorResponse } from '../fixtures/monitor-data.mjs'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  })
  const origin = process.env.QA_BASE_URL || 'http://127.0.0.1:5399'
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-group-alerts-qa'
  await mkdir(output, { recursive: true })
  const errors = []
  const saves = []
  const tests = []
  const policies = new Map()
  let failLoad = false
  let failSave = false
  let failTest = false
  let channels = {
    smtp: { enabled: true, host: 'smtp.example.com', port: 587, security: 'starttls', username: null, passwordSet: false, fromName: 'Synthetic', fromEmail: 'alerts@example.com' },
    bark: { enabled: true, serverUrl: 'https://push.example.com', deviceKeySet: true, level: 'active', sound: null, volume: 5, call: false },
    lastTest: null,
    updatedAt: new Date().toISOString(),
  }
  const defaults = groupId => ({
    groupId,
    enabled: false,
    concurrency: { enabled: true, threshold: 90, confirmationSeconds: 20 },
    eta: { enabled: true, threshold: 10, confirmationSeconds: 30 },
    quotaZero: { enabled: true, threshold: 0, confirmationSeconds: 0 },
    availability: { enabled: true, threshold: 0, confirmationSeconds: 0 },
    emailEnabled: false,
    emailRecipients: ['ops@example.com'],
    barkEnabled: false,
    barkLevel: null,
    barkSound: null,
    barkVolume: null,
    barkCall: null,
    updatedAt: new Date().toISOString(),
  })
  const page = await browser.newPage({ reducedMotion: 'reduce' })
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/dev/api/**', async (route) => {
    const req = route.request()
    const url = new URL(req.url())
    const path = url.pathname.replace(/^\/dev/u, '')
    const ok = data => route.fulfill({ json: { code: 200, message: 'ok', data } })
    if (path === '/api/admin/account-groups/monitor')
      return ok(monitorResponse((url.searchParams.get('groupIds') ?? '').split(',')))
    if (path === '/api/admin/notifications/channels')
      return ok(channels)
    if (path === '/api/admin/notifications/channels/update') {
      const body = req.postDataJSON()
      saves.push(body)
      if (failSave)
        return route.fulfill({ status: 400, json: { code: 400, message: 'Synthetic save failure', data: null } })
      channels = { ...channels, ...body }
      delete channels.smtp.password
      delete channels.bark.deviceKey
      return ok(channels)
    }
    if (path === '/api/admin/account-groups/alert-policy') {
      if (failLoad)
        return route.fulfill({ status: 503, json: { code: 503, message: 'Synthetic read failure', data: null } })
      const id = url.searchParams.get('groupId')
      return ok(policies.get(id) ?? defaults(id))
    }
    if (path === '/api/admin/account-groups/alert-policy/update') {
      const body = req.postDataJSON()
      assert.equal(Object.hasOwn(body, 'updatedAt'), false)
      saves.push(body)
      const saved = { ...body, updatedAt: new Date().toISOString() }
      policies.set(body.groupId, saved)
      return ok(saved)
    }
    if (path === '/api/admin/notifications/test') {
      const body = req.postDataJSON()
      tests.push(body)
      channels.lastTest = {
        id: `test-${tests.length}`,
        groupId: body.groupId ?? null,
        channel: body.channel,
        target: body.target,
        status: failTest ? 'failed' : 'sent',
        test: true,
        attempts: 1,
        error: failTest ? 'delivery failed' : null,
        createdAt: new Date().toISOString(),
        finishedAt: new Date().toISOString(),
      }
      return failTest
        ? route.fulfill({ status: 503, json: { code: 503, message: 'Synthetic delivery failure', data: null } })
        : ok({ id: channels.lastTest.id })
    }
    errors.push(`unexpected request: ${path}`)
    return route.abort()
  })

  try {
    for (const theme of ['light', 'dark']) {
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 900 })
        await page.goto(`${origin}/tests/fixtures/group-alerts.html?theme=${theme}`)
        await page.evaluate(() => localStorage.clear())
        await page.reload()
        const settings = page.locator('#notifications')
        await settings.getByText('尚未测试', { exact: true }).waitFor()
        assert.equal(await settings.locator('fieldset').count(), 0)
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
        await page.screenshot({ path: `${output}/collapsed-${theme}-${width}.png`, fullPage: true })
        await settings.getByRole('button', { name: /通知渠道/u }).click()
        await settings.getByRole('button', { name: /邮件 SMTP/u }).click()
        await settings.getByRole('button', { name: /Bark 推送/u }).click()
        await settings.getByPlaceholder('SMTP 主机').waitFor()
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
        await page.screenshot({ path: `${output}/channels-${theme}-${width}.png`, fullPage: true })
        await page.getByRole('button', { name: `设置 ${groups[0].name} 预警`, exact: true }).click()
        const dialog = page.getByRole('dialog')
        await dialog.getByPlaceholder('收件人，多个用逗号分隔').waitFor()
        await dialog.getByRole('button', { name: /提醒样式覆盖/u }).click()
        await dialog.getByRole('combobox', { name: '响铃方式' }).waitFor()
        assert.equal(await dialog.evaluate(el => el.scrollWidth <= el.clientWidth), true)
        await page.screenshot({ path: `${output}/policy-${theme}-${width}.png`, fullPage: true })
        await dialog.getByRole('button', { name: '取消', exact: true }).click()
      }
    }
    const settings = page.locator('#notifications')
    await settings.getByPlaceholder('SMTP 主机').fill('edited-smtp.example.com')
    await settings.getByPlaceholder('密码', { exact: true }).fill('synthetic-password')
    await settings.getByPlaceholder('https://api.day.app', { exact: true }).fill('https://edited-push.example.com')
    await settings.getByPlaceholder('已保存，留空保持不变', { exact: true }).fill('synthetic-device-key')
    failSave = true
    await settings.getByRole('button', { name: '发送测试邮件', exact: true }).click()
    await page.waitForFunction(() => document.body.textContent.includes('Synthetic save failure'))
    await page.waitForFunction(() => !document.querySelector('#notifications fieldset').disabled)
    assert.equal(await settings.getByPlaceholder('SMTP 主机').inputValue(), 'edited-smtp.example.com')
    assert.equal(await settings.getByPlaceholder('密码', { exact: true }).inputValue(), 'synthetic-password')
    assert.equal(await settings.getByPlaceholder('https://api.day.app', { exact: true }).inputValue(), 'https://edited-push.example.com')
    assert.equal(await settings.getByPlaceholder('已保存，留空保持不变', { exact: true }).inputValue(), 'synthetic-device-key')
    assert.equal(tests.length, 0)
    failSave = false
    await settings.getByRole('button', { name: '发送测试邮件', exact: true }).click()
    await settings.getByText(/SMTP 测试成功/u).waitFor()
    assert.equal(tests.at(-1).target, 'alerts@example.com')
    failTest = true
    await settings.getByRole('button', { name: '发送测试通知', exact: true }).click()
    await settings.getByText(/Bark 测试失败/u).waitFor()
    failTest = false
    await page.getByRole('button', { name: `设置 ${groups[0].name} 预警`, exact: true }).click()
    const dialog = page.getByRole('dialog')
    await dialog.getByRole('button', { name: '保存', exact: true }).click()
    await page.waitForFunction(() => document.body.textContent.includes('分组预警设置已保存'))
    assert.ok(policies.has(groups[0].id))
    await dialog.getByRole('button', { name: '取消', exact: true }).click()
    failLoad = true
    await page.getByRole('button', { name: `设置 ${groups[0].name} 预警`, exact: true }).click()
    await page.waitForFunction(() => document.body.textContent.includes('Synthetic read failure'))
    assert.equal(await dialog.getByRole('button', { name: '保存', exact: true }).isDisabled(), true)
    assert.deepEqual(errors, [])
    process.stdout.write(`${JSON.stringify({ screenshots: 18, saves: saves.length, tests: tests.length, pageErrors: errors })}\n`)
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
