import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const browser = await chromium.launch({ headless: true })
  const base = process.env.QA_BASE_URL || 'http://127.0.0.1:5217'
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-mihomo-ui'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
  const errors = []
  const commands = []
  page.on('pageerror', error => errors.push(error.message))
  const warm = { ready: 3, target: 5, eligible: 5, checking: 1, cooling: 1, readySubscription: 2, readyDynamic: 1, failureReasons: { stream_failure: 1 } }
  const state = {
    version: 'v1.19.31',
    installed: true,
    supported: true,
    running: true,
    busy: false,
    phase: 'idle',
    error: null,
    endpoint: 'http://127.0.0.1:3101',
    subscriptionDownloadMode: 'auto',
    dynamicProxies: 1,
    countryFilter: { mode: 'off', codes: [], allowUnknown: false, dynamicProviderManaged: true },
    countryCodes: ['US', 'GB', 'JP', 'SG', 'DE', 'HK'],
    bpsWarmPool: warm,
    bpsIpWarmPool: warm,
    codexWarmPool: warm,
    codexIpWarmPool: warm,
    subscriptionItems: [{ id: 'fixture-source', label: '测试订阅', enabled: true, nodes: 32, cached: true, updatedAt: '2026-09-28T00:00:00Z' }],
    nodeStates: [
      { name: 'node-fixture', displayName: '东京 01', subscriptionIds: ['fixture-source'], dynamic: false, state: 'ready', countryCode: 'JP', countryBlocked: false, check: null },
      { name: 'DYNAMIC-fixture', displayName: '动态代理 01', subscriptionIds: [], dynamic: true, state: 'ready', countryCode: null, countryBlocked: false, check: null },
    ],
  }
  const check = { checkedAt: '2026-09-28T00:00:00Z', success: true, latencyMs: 35, message: '代理出口连通正常', exitIp: '203.0.113.9', countryCode: 'JP', quality: { score: 90, grade: 'A', summary: '通过 4 项，告警 1 项', checks: [{ name: 'openai', status: 'pass', success: true, latencyMs: 35, reason: 'HTTP 401（目标可达）' }] } }
  await page.route('**/api/admin/**', async (route) => {
    const request = route.request()
    const path = new URL(request.url()).pathname.replace(/^\/dev/, '')
    let data = null
    if (path.endsWith('/auth/status')) {
      data = { authenticated: true }
    }
    else if (path.endsWith('/system/version')) {
      data = { version: 'isolated-mihomo-ui', buildType: 'test' }
    }
    else if (path.endsWith('/proxies/mihomo/check')) {
      data = check
    }
    else if (path.endsWith('/proxies/mihomo')) {
      if (request.method() === 'POST')
        commands.push(request.postDataJSON())
      data = state
    }
    else if (path.endsWith('/proxies')) {
      data = { items: [], page: { page: 1, pageSize: 20, total: 0, totalPages: 0 } }
    }
    await route.fulfill({ json: { code: 200, message: 'ok', data } })
  })
  try {
    await page.goto(`${base}/proxies`)
    await page.getByRole('button', { name: '订阅管理', exact: true }).click()
    await page.getByText('测试订阅', { exact: true }).waitFor()
    await page.getByRole('textbox', { name: '订阅地址（每行一个）', exact: true }).fill('https://fixture.invalid/subscription')
    await page.getByRole('button', { name: '添加订阅', exact: true }).click()
    await page.waitForFunction(() => document.querySelector('textarea')?.value === '')
    assert.equal(commands.at(-1).action, 'subscription_add')
    await page.getByRole('textbox', { name: '订阅地址（每行一个）', exact: true }).fill('do-not-copy-between-tabs')
    await page.getByRole('button', { name: '动态代理', exact: true }).click()
    assert.equal(await page.getByRole('textbox', { name: '动态代理（每行一个）', exact: true }).inputValue(), '')
    await page.getByRole('textbox', { name: '动态代理（每行一个）', exact: true }).fill('proxy.example:8000:user:password')
    await page.getByRole('button', { name: '追加', exact: true }).click()
    await page.waitForFunction(() => document.querySelector('textarea')?.value === '')
    assert.equal(commands.at(-1).action, 'dynamic_append')
    await page.getByRole('button', { name: '节点管理', exact: true }).click()
    await page.getByText('东京 01', { exact: true }).waitFor()
    await page.getByRole('button', { name: '完整质量检测', exact: true }).first().click()
    await page.getByRole('dialog').getByText('A · 90 分', { exact: true }).waitFor()
    await page.keyboard.press('Escape')
    await page.getByRole('dialog').waitFor({ state: 'hidden' })
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: width < 500 ? 844 : 1000 })
      for (const [tab, filename] of [['节点管理', 'nodes'], ['订阅管理', 'sources'], ['内核与规则', 'kernel']]) {
        await page.getByRole('button', { name: tab, exact: true }).click()
        await page.getByText('Mihomo v1.19.31', { exact: true }).waitFor()
        if (filename === 'kernel') {
          assert.equal(await page.getByText('允许未知地区', { exact: true }).isVisible(), true)
          assert.equal(await page.getByText('动态代理地区由供应商管理', { exact: true }).isVisible(), true)
          assert.equal(await page.getByText('US', { exact: true }).isVisible(), true)
        }
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), `${tab} overflow at ${width}`)
        await page.screenshot({ path: `${output}/${filename}-${width}.png`, fullPage: true })
      }
    }
    await page.getByRole('button', { name: '保存地区规则', exact: true }).click()
    await page.waitForTimeout(100)
    assert.equal(commands.at(-1).action, 'country_filter')
    assert.deepEqual(errors, [])
    process.stdout.write(`${JSON.stringify({ commands: commands.map(c => c.action), screenshots: 6, errors })}\n`)
  }
  finally { await browser.close() }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
