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
    bark: { enabled: true, serverUrl: 'https://push.example.com', deviceKeySet: true, level: 'critical', sound: null, volume: 5, call: false },
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
      channels = {
        ...channels,
        ...body,
        smtp: { ...body.smtp, passwordSet: channels.smtp.passwordSet || Boolean(body.smtp.password) },
        bark: { ...body.bark, deviceKeySet: channels.bark.deviceKeySet || Boolean(body.bark.deviceKey) },
      }
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
        await settings.getByRole('button', { name: /SMTP 发件配置/u }).click()
        await settings.getByRole('button', { name: /Bark iPhone 提醒/u }).click()
        for (const label of ['SMTP 服务器地址', 'SMTP 端口', '登录账号（可选）', '密码 / 授权码', '发件人地址 (From)', '发件人显示名称（可选）', '加密方式', '测试收件地址', 'Bark Server（服务器地址）', 'Device Key（设备密钥）', '提醒等级', '铃声名称（可选）', '重要警告音量'])
          await settings.getByLabel(label, { exact: true }).waitFor()
        await settings.getByText(/需在 iPhone 为 Bark 开启「重要警告」/u).waitFor()
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
        await page.screenshot({ path: `${output}/channels-${theme}-${width}.png`, fullPage: true })
        await page.getByRole('button', { name: `设置 ${groups[0].name} 预警`, exact: true }).click()
        const dialog = page.getByRole('dialog')
        await dialog.getByLabel('收件地址', { exact: true }).waitFor()
        await dialog.getByText('继承全局 · 重要警告（静音仍响铃）', { exact: true }).waitFor()
        await dialog.getByRole('button', { name: /提醒等级与铃声/u }).click()
        await dialog.getByRole('combobox', { name: '响铃时长' }).waitFor()
        await dialog.getByText(/需在 iPhone 为 Bark 开启「重要警告」/u).waitFor()
        assert.equal(await dialog.evaluate(el => el.scrollWidth <= el.clientWidth), true)
        await page.screenshot({ path: `${output}/policy-${theme}-${width}.png`, fullPage: true })
        await dialog.getByRole('switch', { name: '自定义重要警告音量', exact: true }).scrollIntoViewIfNeeded()
        await page.screenshot({ path: `${output}/policy-bark-${theme}-${width}.png` })
        await dialog.getByRole('button', { name: '取消', exact: true }).click()
      }
    }
    const settings = page.locator('#notifications')
    const choose = async (container, label, option) => {
      const trigger = container.getByRole('combobox', { name: label, exact: true })
      await trigger.click()
      const listbox = page.locator(`[id="${await trigger.getAttribute('aria-controls')}"]`)
      await listbox.getByRole('option', { name: option, exact: true }).click()
    }
    for (const [label, hint] of [
      ['普通提醒', '跟随手机静音设置'],
      ['时效性提醒', '可在专注模式下显示'],
      ['静默记录', '仅进入通知列表'],
      ['重要警告（静音仍响铃）', '需在 iPhone 为 Bark 开启「重要警告」'],
    ]) {
      await choose(settings, '提醒等级', label)
      await settings.getByText(hint, { exact: false }).waitFor()
    }
    await settings.getByRole('switch', { name: '持续响铃约 30 秒', exact: true }).press('Space')
    await settings.getByText(/持续响铃只延长时长/u).waitFor()
    await settings.getByLabel('SMTP 服务器地址', { exact: true }).fill('edited-smtp.example.com')
    await settings.getByLabel('登录账号（可选）', { exact: true }).fill('login@example.com')
    await settings.getByLabel('发件人地址 (From)', { exact: true }).fill('sender@example.com')
    await settings.getByLabel('发件人显示名称（可选）', { exact: true }).fill('Synthetic sender')
    await settings.getByLabel('测试收件地址', { exact: true }).fill('test@example.com')
    await settings.getByLabel('密码 / 授权码', { exact: true }).fill('synthetic-password')
    await settings.getByLabel('Bark Server（服务器地址）', { exact: true }).fill('https://edited-push.example.com')
    await settings.getByLabel('Device Key（设备密钥）', { exact: true }).fill('synthetic-device-key')
    failSave = true
    await settings.getByRole('button', { name: '发送测试邮件', exact: true }).click()
    await page.waitForFunction(() => document.body.textContent.includes('Synthetic save failure'))
    await page.waitForFunction(() => !document.querySelector('#notifications fieldset').disabled)
    assert.equal(await settings.getByLabel('SMTP 服务器地址', { exact: true }).inputValue(), 'edited-smtp.example.com')
    assert.equal(await settings.getByLabel('密码 / 授权码', { exact: true }).inputValue(), 'synthetic-password')
    assert.equal(await settings.getByLabel('Bark Server（服务器地址）', { exact: true }).inputValue(), 'https://edited-push.example.com')
    assert.equal(await settings.getByLabel('Device Key（设备密钥）', { exact: true }).inputValue(), 'synthetic-device-key')
    assert.equal(tests.length, 0)
    failSave = false
    await settings.getByRole('button', { name: '发送测试邮件', exact: true }).click()
    await settings.getByText(/SMTP 测试成功/u).waitFor()
    assert.equal(tests.at(-1).target, 'test@example.com')
    assert.equal(saves.at(-1).smtp.username, 'login@example.com')
    assert.equal(saves.at(-1).smtp.fromEmail, 'sender@example.com')
    assert.equal(saves.at(-1).smtp.fromName, 'Synthetic sender')
    assert.equal(saves.at(-1).smtp.security, 'starttls')
    assert.equal(saves.at(-1).smtp.port, 587)
    assert.equal(saves.at(-1).bark.level, 'critical')
    assert.equal(saves.at(-1).bark.call, true)
    assert.equal(await settings.getByLabel('密码 / 授权码', { exact: true }).getAttribute('placeholder'), '已保存，留空保持不变')
    assert.equal(await settings.getByLabel('密码 / 授权码', { exact: true }).inputValue(), '')
    failTest = true
    await settings.getByRole('button', { name: '发送测试通知', exact: true }).click()
    await settings.getByText(/Bark 测试失败/u).waitFor()
    failTest = false
    await page.getByRole('button', { name: `设置 ${groups[0].name} 预警`, exact: true }).click()
    const dialog = page.getByRole('dialog')
    const savePolicy = async () => {
      const response = page.waitForResponse(result => result.url().endsWith('/account-groups/alert-policy/update'))
      await dialog.getByRole('button', { name: '保存', exact: true }).click()
      await response
      await page.waitForFunction(() => !document.querySelector('[role="dialog"] fieldset').disabled)
    }
    await dialog.getByLabel('收件地址', { exact: true }).waitFor()
    await savePolicy()
    await page.waitForFunction(() => document.body.textContent.includes('分组预警设置已保存'))
    assert.ok(policies.has(groups[0].id))
    for (const key of ['barkLevel', 'barkSound', 'barkVolume', 'barkCall'])
      assert.equal(policies.get(groups[0].id)[key], null)
    const advanced = dialog.getByRole('button', { name: /提醒等级与铃声/u })
    if (await advanced.getAttribute('aria-expanded') === 'false')
      await advanced.click()
    await choose(dialog, '提醒等级', '静默记录')
    await dialog.getByText('仅进入通知列表，不亮屏、不响铃。', { exact: true }).waitFor()
    await choose(dialog, '提醒等级', '重要警告（静音仍响铃）')
    await dialog.getByText(/需在 iPhone 为 Bark 开启「重要警告」/u).waitFor()
    await choose(dialog, '响铃时长', '持续响铃约 30 秒')
    await dialog.getByRole('switch', { name: '自定义重要警告音量', exact: true }).press('Space')
    await dialog.getByRole('spinbutton', { name: '重要警告音量', exact: true }).fill('8')
    await dialog.getByLabel('铃声名称', { exact: true }).fill('alarm')
    await dialog.getByLabel('收件地址', { exact: true }).fill('ops@example.com, backup@example.com')
    await savePolicy()
    assert.equal(policies.get(groups[0].id).barkLevel, 'critical')
    assert.equal(policies.get(groups[0].id).barkVolume, 8)
    assert.equal(policies.get(groups[0].id).barkCall, true)
    assert.equal(policies.get(groups[0].id).barkSound, 'alarm')
    assert.deepEqual(policies.get(groups[0].id).emailRecipients, ['ops@example.com', 'backup@example.com'])
    await choose(dialog, '提醒等级', '继承全局')
    await choose(dialog, '响铃时长', '继承全局')
    await dialog.getByRole('switch', { name: '自定义重要警告音量', exact: true }).press('Space')
    await savePolicy()
    for (const key of ['barkLevel', 'barkVolume', 'barkCall'])
      assert.equal(policies.get(groups[0].id)[key], null)
    await dialog.getByRole('button', { name: '取消', exact: true }).click()
    failLoad = true
    await page.getByRole('button', { name: `设置 ${groups[0].name} 预警`, exact: true }).click()
    await page.waitForFunction(() => document.body.textContent.includes('Synthetic read failure'))
    assert.equal(await dialog.getByRole('button', { name: '保存', exact: true }).isDisabled(), true)
    assert.deepEqual(errors, [])
    process.stdout.write(`${JSON.stringify({ screenshots: 24, saves: saves.length, tests: tests.length, pageErrors: errors })}\n`)
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
