/* eslint-disable no-console -- standalone browser regression runner. */
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

async function main() {
  const playwright = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const browser = await playwright.chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-post-release-settings-qa'
  await mkdir(output, { recursive: true })
  const now = new Date().toISOString()
  const originalUa = 'codex_cli_rs/0.146.0 (Linux 6.8.0; x86_64) unknown'
  let settings = {
    modelMappings: {},
    refreshMarginSeconds: 300,
    refreshConcurrency: 4,
    maxConcurrentPerAccount: 5,
    requestIntervalMs: 0,
    openaiSessionBindingTtlHours: 24,
    rotationStrategy: 'smart',
    minCodexDesktopVersion: null,
    minCodexCliVersion: null,
    usageRetentionDays: 31,
    opsEventRetentionDays: 30,
    auditRetentionDays: 90,
    updatedAt: now,
  }
  let ua = {
    mode: 'custom',
    customUserAgent: originalUa,
    defaultUserAgent: originalUa,
    effectiveUserAgent: originalUa,
    effectiveDesktopUserAgent: 'Codex Desktop/26.901.51231 (Linux; x86_64)',
    coreVersion: '0.146.0',
    desktopVersion: '26.901.51231',
    osType: 'Linux',
    osVersion: '6.8.0',
    arch: 'x86_64',
    terminal: 'unknown',
    verified: false,
    defaultVerifiedAt: now,
  }
  const errors = []
  const writes = []
  const unexpected = []
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
    page.on('pageerror', error => errors.push(error.message))
    await page.route('**/dev/api/**', async (route) => {
      const request = route.request()
      const path = new URL(request.url()).pathname.replace('/dev', '')
      let data
      if (path === '/api/admin/auth/refresh' || path === '/api/admin/auth/status') {
        data = { authenticated: true, expiresAt: new Date(Date.now() + 3600000).toISOString() }
      }
      else if (path === '/api/admin/settings') {
        data = settings
      }
      else if (path === '/api/admin/settings/update') {
        const body = request.postDataJSON()
        writes.push(body)
        settings = { ...body, updatedAt: now }
        data = settings
      }
      else if (path === '/api/admin/settings/admin-api-key') {
        data = { exists: false }
      }
      else if (path === '/api/admin/settings/openai-user-agent') {
        if (request.method() === 'POST') {
          const body = request.postDataJSON()
          writes.push(body)
          ua = { ...ua, ...body, customUserAgent: body.userAgent, effectiveUserAgent: body.userAgent, osVersion: '6.12.8' }
        }
        data = ua
      }
      else if (path === '/api/admin/system/version') {
        data = { version: 'local-test', buildType: 'test' }
      }
      else if (path === '/api/admin/notifications/channels') {
        data = {
          smtp: { enabled: false, host: '', port: 465, security: 'tls', username: null, passwordSet: false, fromName: null, fromEmail: null },
          bark: { enabled: false, serverUrl: 'https://notifications.example.com', deviceKeySet: false, level: 'active', sound: null, volume: 5, call: false },
          lastTest: null,
          updatedAt: now,
        }
      }
      else {
        unexpected.push(`${request.method()} ${path}`)
        data = null
      }
      await route.fulfill({ json: { code: 200, message: 'ok', data } })
    })
    await page.goto(`${process.env.QA_BASE_URL || 'http://127.0.0.1:5177'}/settings`)
    const ttl = page.getByRole('spinbutton', { name: 'OpenAI 会话账号绑定时长（小时）', exact: true })
    await ttl.waitFor()
    assert.equal(await ttl.inputValue(), '24')
    const affinity = page.getByRole('radiogroup', { name: 'OpenAI 账号亲和模式', exact: true })
    assert.equal(await affinity.getByRole('radio', { name: '严格', exact: true }).getAttribute('aria-checked'), 'true')
    await affinity.getByRole('radio', { name: '宽松', exact: true }).click()
    await ttl.fill('48')
    await page.getByRole('button', { name: '保存全部设置', exact: true }).click()
    await page.getByText('设置已保存', { exact: true }).waitFor()
    assert.equal(writes[0].openaiSessionBindingTtlHours, 48)
    assert.equal(writes[0].openaiAccountAffinity, 'relaxed')
    assert.equal(writes[0].rotationStrategy, 'smart')
    const os = page.getByRole('textbox', { name: 'OpenAI 出站系统版本', exact: true })
    await os.fill('6.12.8')
    await page.getByRole('button', { name: '保存设置', exact: true }).click()
    await page.getByText('出站设置已保存', { exact: true }).waitFor()
    assert.equal(writes[1].userAgent, originalUa.replace('6.8.0', '6.12.8'))
    assert.equal(writes[1].mode, 'custom')
    assert.equal(Object.hasOwn(writes[1], 'tlsProfile'), false)
    assert.equal(Object.hasOwn(writes[1], 'sessionPolicy'), false)
    await page.reload()
    await ttl.waitFor()
    assert.equal(await ttl.inputValue(), '48')
    assert.equal(await affinity.getByRole('radio', { name: '宽松', exact: true }).getAttribute('aria-checked'), 'true')
    assert.equal(await os.inputValue(), '6.12.8')
    for (const [name, width, height] of [['desktop', 1440, 1000], ['mobile', 390, 844]]) {
      await page.setViewportSize({ width, height })
      await affinity.getByRole('radio', { name: '严格', exact: true }).click()
      const saved = page.waitForResponse(response => response.url().endsWith('/api/admin/settings/update') && response.request().method() === 'POST')
      await page.getByRole('button', { name: '保存全部设置', exact: true }).click()
      assert.equal((await saved).status(), 200)
      await page.reload()
      await ttl.waitFor()
      assert.equal(await affinity.getByRole('radio', { name: '严格', exact: true }).getAttribute('aria-checked'), 'true')
      await ttl.scrollIntoViewIfNeeded()
      await page.screenshot({ path: `${output}/${name}-runtime.png` })
      await os.scrollIntoViewIfNeeded()
      await page.screenshot({ path: `${output}/${name}-os.png` })
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1)
      assert.equal(overflow, false, `${name} overflow`)
    }
    assert.deepEqual(errors, [])
    assert.deepEqual(unexpected, [])
    console.log(JSON.stringify({ screenshots: output, writes: writes.length, errors }))
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
