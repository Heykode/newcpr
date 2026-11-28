/* eslint-disable no-console -- Standalone isolated browser validation. */
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const browser = await chromium.launch({ headless: true })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-native-platform-settings'
  await mkdir(output, { recursive: true })
  const server = await createServer({ root: fileURLToPath(new URL('../..', import.meta.url)), server: { host: '127.0.0.1', port: 0 }, plugins: [{ name: 'isolated-native-settings', configResolved(config) {
    config.server.proxy = {}
  } }] })
  await server.listen()
  const base = `http://127.0.0.1:${server.httpServer.address().port}`
  const now = new Date().toISOString()
  const errors = []
  const unexpected = []
  const writes = []
  const agy = { client: 'desktop', platform: 'linux', versionMode: 'latest', clientVersion: null, arch: 'x64', systemPrompt: null }
  let settings = { configRevision: 1, openaiRequestSettings: { requestCountry: { mode: 'follow_location' }, oauthGuardianReviewModel: true, residency: 'us', accountRoutingOverride: null }, openaiWebsocketPool: { enabled: true, maxAgeMs: 3300000, fastPathBudgetMs: 800, idleTimeoutMs: 300000, streamIdleTimeoutMs: 300000 }, modelMappings: {}, refreshMarginSeconds: 300, refreshConcurrency: 4, maxConcurrentPerAccount: 5, requestIntervalMs: 0, sessionAffinityTtlHours: 24, accountAffinity: 'strict', rotationStrategy: 'smart', usageRetentionDays: 31, opsEventRetentionDays: 30, auditRetentionDays: 90, providerRequestProfiles: {}, codexPrivacyPolicy: { enabled: false, onError: 'skip_rule', rules: [] }, updatedAt: now }
  const price = { multiplierBps: 10000, bands: { standard: { input: '1', output: '2', cacheRead: '0.1', cacheWrite: '0' } } }
  const pricing = { defaults: { openai: { 'fixture-model': price }, antigravity: { 'fixture-gemini': price } }, synced: {}, overrides: {}, supportedBands: { openai: ['standard', 'fast', 'ultrafast', 'image'], xai: ['standard'], antigravity: ['standard'] }, syncedAt: null, syncedAtDisplay: null }
  let page
  try {
    page = await browser.newPage({ reducedMotion: 'reduce', viewport: { width: 1440, height: 1000 } })
    page.on('pageerror', error => errors.push(error.message))
    await page.route('**/dev/api/**', async (route) => {
      const req = route.request()
      const url = new URL(req.url())
      const path = url.pathname.replace('/dev', '')
      let data
      if (path.startsWith('/api/admin/auth/')) {
        data = { authenticated: true, expiresAt: new Date(Date.now() + 3600000).toISOString() }
      }
      else if (path === '/api/admin/system/update/detail') {
        data = { enabled: false, status: 'idle', currentVersion: 'isolated-test' }
      }
      else if (path === '/api/admin/system/version') {
        data = { version: 'isolated-test', buildType: 'test' }
      }
      else if (path === '/api/admin/settings') {
        data = settings
      }
      else if (path === '/api/admin/settings/update') {
        const body = req.postDataJSON()
        assert.equal(body.configRevision, settings.configRevision)
        writes.push(body)
        settings = { ...settings, ...body, configRevision: settings.configRevision + 1, providerRequestProfiles: { ...settings.providerRequestProfiles, ...body.providerRequestProfiles } }
        data = settings
      }
      else if (path === '/api/admin/settings/admin-api-key') {
        data = { exists: false }
      }
      else if (path === '/api/admin/settings/openai-user-agent') {
        data = { mode: 'default', defaultUserAgent: 'fixture-cli/0.0.0', effectiveUserAgent: 'fixture-cli/0.0.0', effectiveDesktopUserAgent: 'Fixture Desktop/0.0.0', coreVersion: '0.0.0', desktopVersion: '0.0.0', osType: 'Linux', osVersion: 'fixture', arch: 'x64', terminal: 'fixture', verified: false, defaultVerifiedAt: null }
      }
      else if (path === '/api/admin/notifications/channels') {
        data = { smtp: { enabled: false, host: '', port: 465, security: 'tls', username: null, passwordSet: false, fromName: null, fromEmail: null }, bark: { enabled: false, serverUrl: 'https://notifications.example.com', deviceKeySet: false, level: 'active', sound: null, volume: 5, call: false }, lastTest: null, updatedAt: now }
      }
      else if (path === '/api/admin/settings/client-profiles') {
        assert.equal(url.searchParams.get('provider'), 'antigravity')
        data = { defaults: agy, globalConfiguration: settings.providerRequestProfiles.antigravity ?? agy, presets: [{ configuration: agy, automaticAvailable: true, reason: null }], defaultSystemPrompt: 'Synthetic server instruction', systemPromptSource: 'fixture' }
      }
      else if (path === '/api/admin/settings/client-profiles/preview') {
        data = { source: 'global', userAgent: 'fixture-agy/0.0.0', versionSource: 'official', verifiedAt: null, checkedAt: null, error: null, configuration: req.postDataJSON().configuration ?? agy, product: 'fixture', osType: 'Linux', arch: 'x64', clientVersion: '0.0.0' }
      }
      else if (path === '/api/admin/settings/privacy/preview') {
        const body = req.postDataJSON()
        data = { body: body.body, headers: body.headers, turnMetadata: body.turnMetadata, outcomes: body.policy.rules.map(rule => ({ ruleId: rule.id, matches: 1, status: 'applied', reason: null })) }
      }
      else if (path === '/api/admin/settings/pricing') {
        data = pricing
      }
      else if (path === '/api/admin/settings/pricing/update') {
        const body = req.postDataJSON()
        writes.push(body)
        pricing.overrides[body.provider] ??= {}
        for (const model of body.models) pricing.overrides[body.provider][model] = body.change.pricing
        data = { saved: true }
      }
      else {
        unexpected.push(`${req.method()} ${path}`)
        return route.abort()
      }
      await route.fulfill({ json: { code: 200, message: 'ok', data } })
    })
    await page.goto(`${base}/settings`)
    const scope = page.getByRole('radiogroup', { name: '配置范围', exact: true })
    const identity = page.getByRole('region', { name: '平台客户端身份', exact: true })
    await identity.getByRole('radio', { name: 'Antigravity', exact: true }).click()
    const prompt = identity.getByRole('textbox', { name: '系统提示词', exact: true })
    await prompt.fill('Synthetic edited server instruction')
    await Promise.all([page.waitForResponse(response => response.url().endsWith('/api/admin/settings/update') && response.status() === 200), page.getByRole('button', { name: '保存全部设置', exact: true }).click()])
    assert.equal(settings.providerRequestProfiles.antigravity.systemPrompt, 'Synthetic edited server instruction')
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 })
      await scope.getByRole('radio', { name: '通用与 Codex 配置', exact: true }).click()
      assert.equal(await scope.getByRole('radio', { name: 'Excel 配置', exact: true }).count(), 0)
      const common = page.getByRole('region', { name: '通用与 Codex 配置', exact: true })
      await common.getByRole('textbox', { name: '数据驻留', exact: true }).fill('')
      await common.getByRole('textbox', { name: '账号路由', exact: true }).fill(width === 390 ? '' : 'us_cr')
      const values = { 连接最大寿命: 66000, 建连等待时间: 432, 空闲回收时间: 77000, 流式空闲超时: 0 }
      for (const [label, value] of Object.entries(values))
        await common.getByRole('spinbutton', { name: label, exact: true }).fill(String(value))
      await Promise.all([page.waitForResponse(response => response.url().endsWith('/api/admin/settings/update') && response.status() === 200), page.getByRole('button', { name: '保存全部设置', exact: true }).click()])
      assert.equal(settings.openaiRequestSettings.residency, null)
      assert.equal(settings.openaiRequestSettings.accountRoutingOverride, width === 390 ? null : 'us_cr')
      assert.deepEqual(settings.openaiWebsocketPool, { enabled: true, maxAgeMs: 66000, fastPathBudgetMs: 432, idleTimeoutMs: 77000, streamIdleTimeoutMs: 0 })
      assert(!('websocketMaxAgeMs' in settings.requestTuning))
      await page.reload()
      await common.locator('input[aria-label="连接最大寿命"]:enabled').waitFor()
      assert.equal(await common.getByRole('textbox', { name: '数据驻留', exact: true }).inputValue(), '')
      assert.equal(await common.getByRole('spinbutton', { name: '建连等待时间', exact: true }).inputValue(), '432')
      await common.getByRole('spinbutton', { name: '建连等待时间', exact: true }).scrollIntoViewIfNeeded()
      await page.screenshot({ path: `${output}/openai-pool-${width}.png` })
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), true)
      await scope.getByRole('radio', { name: '工作区隐私', exact: true }).click()
      await page.getByRole('button', { name: '添加规则', exact: true }).click()
      const dialog = page.getByRole('dialog')
      await dialog.getByRole('textbox', { name: '规则名称', exact: true }).fill(`Fixture ${width}`)
      await dialog.getByRole('textbox', { name: '字段路径', exact: true }).fill('$.metadata.fixture')
      await dialog.getByRole('button', { name: '测试本条规则', exact: true }).click()
      await dialog.getByText('命中 1 处', { exact: true }).waitFor()
      await page.screenshot({ path: `${output}/privacy-${width}.png` })
      assert.equal(await dialog.evaluate(el => el.scrollWidth <= el.clientWidth + 1), true)
      await dialog.getByRole('button', { name: '应用到草稿', exact: true }).click()
      await dialog.waitFor({ state: 'hidden' })
      await Promise.all([page.waitForResponse(response => response.url().endsWith('/api/admin/settings/update') && response.status() === 200), page.getByRole('button', { name: '保存全部设置', exact: true }).click()])
      assert(settings.codexPrivacyPolicy.rules.some(rule => rule.name === `Fixture ${width}`))
      await scope.getByRole('radio', { name: '模型价目', exact: true }).click()
      await page.getByRole('button', { name: '添加模型', exact: true }).click()
      const priceDialog = page.getByRole('dialog')
      await priceDialog.getByRole('textbox', { name: '上游模型 ID', exact: true }).fill(`fixture-${width}`)
      for (const label of ['输入单价', '输出单价', '缓存读取单价', '缓存写入单价'])
        await priceDialog.getByRole('textbox', { name: label, exact: true }).fill('1.25')
      for (const label of ['输入单价', '输出单价', '缓存读取单价', '缓存写入单价']) {
        const visible = await priceDialog.getByRole('textbox', { name: label, exact: true }).evaluate((input) => {
          const style = getComputedStyle(input)
          const context = document.createElement('canvas').getContext('2d')
          context.font = style.font
          const available = input.clientWidth - Number.parseFloat(style.paddingLeft) - Number.parseFloat(style.paddingRight)
          return available >= context.measureText(input.value).width
        })
        assert.ok(visible, `${width}: price value is fully visible beside its unit`)
      }
      const invalidLabels = await page.locator('label[for]').evaluateAll(labels => labels.filter((label) => {
        const target = document.getElementById(label.htmlFor)
        return target && target.getAttribute('role') === 'radiogroup'
      }).length)
      assert.equal(invalidLabels, 0)
      await page.screenshot({ path: `${output}/pricing-${width}.png` })
      assert.equal(await priceDialog.evaluate(el => el.scrollWidth <= el.clientWidth + 1), true)
      await priceDialog.getByRole('button', { name: '保存', exact: true }).click()
      await priceDialog.waitFor({ state: 'hidden' })
      assert.equal(pricing.overrides.openai[`fixture-${width}`].bands.standard.input, '1.25')
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), true)
    }
    assert.deepEqual(errors, [])
    assert.deepEqual(unexpected, [])
    console.log(JSON.stringify({ widths: [1440, 390, 320], writes: writes.length, errors, unexpected }))
  }
  catch (error) {
    if (page) {
      await page.screenshot({ path: `${output}/failure.png`, fullPage: true })
      console.error(JSON.stringify({ errors, unexpected, writes, pageText: await page.locator('body').textContent() }))
    }
    throw error
  }
  finally {
    await browser.close()
    await server.close()
  }
}
main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
