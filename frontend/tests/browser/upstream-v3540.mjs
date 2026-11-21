import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'

async function main() {
  const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href)
  const output = process.env.QA_OUTPUT_DIR || '/tmp/newcpr-v3540-browser'
  await mkdir(output, { recursive: true })
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    server: { host: '127.0.0.1', port: 43156, strictPort: true },
    plugins: [{ name: 'isolated-v3540', configResolved(config) { config.server.proxy = {} } }],
  })
  await server.listen()
  const base = `http://127.0.0.1:${server.httpServer.address().port}`
  const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH })
  try {
    for (const width of [1440, 390]) {
      const page = await browser.newPage({ locale: 'zh-CN', viewport: { width, height: 1000 } })
      await page.context().routeWebSocket('**/*', (socket) => {
        assert.equal(new URL(socket.url()).host, new URL(base).host, 'only local HMR sockets permitted')
        socket.close()
      })
      const errors = []
      const writes = []
      const queries = []
      const key = {
        id: 'key_fixture',
        name: '验收密钥',
        label: null,
        prefix: 'fixture',
        enabled: true,
        maxConcurrency: 7,
        requestsPerMinute: 11,
        dailyLimitUsd: '10',
        weeklyLimitUsd: '50',
        dailyUsedUsd: '2',
        weeklyUsedUsd: '7',
        dailyResetsAt: null,
        weeklyResetsAt: null,
        createdAt: '2026-11-01',
        updatedAt: '2026-11-01',
        lastUsedAt: null,
        routingScope: 'all',
        groups: [],
        providerKinds: ['openai'],
        providerRequestProfileOverrides: {},
      }
      const totals = { requests: 5, tokens: 80, costUsd: '2', hasMissingCost: false, hasEstimatedCost: true }
      const settings = {
        configRevision: 1,
        rotationStrategy: 'smart',
        refreshMarginSeconds: 300,
        refreshConcurrency: 2,
        maxConcurrentPerAccount: 3,
        requestIntervalMs: 0,
        usageRetentionDays: 31,
        opsEventRetentionDays: 30,
        auditRetentionDays: 90,
        requestTuning: {},
        modelMappings: {},
        openaiOauthGuardianReviewModel: true,
        openaiRequestCountry: { mode: 'follow_location' },
      }
      page.on('pageerror', error => errors.push(error.message))
      await page.route('**/*', async (route) => {
        const req = route.request()
        const url = new URL(req.url())
        assert.equal(url.origin, base, 'synthetic loopback traffic only')
        const path = url.pathname.replace(/^\/dev/, '')
        if (!path.startsWith('/api/'))
          return route.continue()
        queries.push({ path, params: Object.fromEntries(url.searchParams) })
        if (req.method() !== 'GET')
          writes.push({ path, body: req.postDataJSON() })
        let data = []
        if (path.endsWith('/auth/status'))
          data = { authenticated: true }
        else if (path.endsWith('/system/version'))
          data = { version: 'synthetic', buildType: 'test' }
        else if (path.endsWith('/settings'))
          data = settings
        else if (path.endsWith('/client-keys'))
          data = { items: [key], total: 1, nextCursor: null }
        else if (path.endsWith('/client-keys/batch-update'))
          data = { updated: 1, configRevision: 2 }
        else if (path.endsWith('/statistics/models'))
          data = [{ model: 'fixture-model', totals, activeDays: 1, peakRequests: 5 }]
        else if (path.endsWith('/statistics'))
          data = { dates: ['2026-11-15', '2026-11-16', '2026-11-17', '2026-11-18', '2026-11-19', '2026-11-20', '2026-11-21'], asOf: 1795219200000, budget: key, modelCount: 1, totals, series: [{ model: 'fixture-model', points: [{ date: '2026-11-21', value: '2' }] }] }
        else if (path.endsWith('/budget-reference-accounts'))
          data = { items: [{ id: 'acct_fixture', name: '参考账号', email: 'reference@example.invalid', provider: 'openai' }], nextCursor: null }
        else if (path.endsWith('/budget-alignment'))
          data = req.method() === 'POST' ? 4 : { revision: 3, windows: [{ key: 'weekly', label: '周额度', period: 'weekly', startsAt: '2026-11-18T00:00:00Z', endsAt: '2026-11-25T00:00:00Z', startsAtDisplay: '2026-11-18 08:00', endsAtDisplay: '2026-11-25 08:00' }] }
        else if (path.endsWith('/account-groups/models'))
          data = { models: [{ id: 'fixture-model', label: 'Fixture model' }] }
        else if (path.endsWith('/account-groups'))
          data = { items: [], page: { page: 1, pageSize: 20, total: 0, totalPages: 0 }, configRevision: 1 }
        else if (path.endsWith('/account-groups/create'))
          data = { id: 'grp_fixture', record: null, configRevision: 2 }
        return route.fulfill({ json: { code: 200, message: 'ok', data } })
      })
      await page.goto(`${base}/api-keys`)
      await page.getByRole('button', { name: '更多密钥操作' }).waitFor()
      await page.getByRole('checkbox').first().locator('..').click()
      await page.getByRole('button', { name: /批量编辑/ }).click()
      let dialog = page.getByRole('dialog', { name: '批量编辑 API Key' })
      await dialog.getByRole('checkbox', { name: '最大并发', exact: true }).locator('..').click()
      await dialog.getByRole('textbox', { name: '最大并发', exact: true }).fill('9')
      await page.screenshot({ path: `${output}/batch-${width}.png` })
      await dialog.getByRole('button', { name: /保存/ }).click()
      await dialog.waitFor({ state: 'hidden' })
      assert.deepEqual(writes.find(item => item.path.endsWith('/batch-update')).body, { ids: ['key_fixture'], maxConcurrency: 9 })
      await page.getByRole('button', { name: '更多密钥操作' }).click()
      await page.getByRole('menuitem', { name: '用量统计' }).click()
      dialog = page.getByRole('dialog')
      await dialog.getByText('fixture-model', { exact: true }).first().waitFor()
      await page.screenshot({ path: `${output}/statistics-${width}.png` })
      assert.equal(queries.find(item => item.path.endsWith('/statistics/models')).params.asOf, '1795219200000')
      await page.keyboard.press('Escape')
      await dialog.waitFor({ state: 'hidden' })
      await page.getByRole('button', { name: '更多密钥操作' }).click()
      await page.getByRole('menuitem', { name: '对齐本期预算窗口' }).click()
      dialog = page.getByRole('dialog', { name: '对齐本期预算窗口' })
      await dialog.getByRole('combobox').first().click()
      await page.getByRole('option', { name: /参考账号/ }).click()
      await dialog.getByText('2026-11-25 08:00', { exact: true }).waitFor()
      await page.screenshot({ path: `${output}/alignment-${width}.png` })
      await dialog.getByRole('button', { name: '确认对齐' }).click()
      await dialog.waitFor({ state: 'hidden' })
      const alignment = writes.find(item => item.path.endsWith('/budget-alignment')).body
      assert.equal(alignment.revision, 3)
      assert.equal(alignment.accountId, 'acct_fixture')
      await page.goto(`${base}/account-groups`)
      await page.getByRole('button', { name: '创建分组', exact: true }).click()
      dialog = page.getByRole('dialog', { name: '创建分组', exact: true })
      await dialog.getByRole('textbox', { name: '分组名称', exact: true }).fill('分组策略验收')
      await dialog.getByRole('combobox').click()
      await page.getByRole('option', { name: 'Ultrafast', exact: true }).click()
      await dialog.getByRole('radio', { name: '白名单', exact: true }).click()
      await dialog.getByRole('button', { name: '加入图片模型', exact: true }).click()
      await dialog.getByRole('switch', { name: '允许子代理', exact: true }).locator('..').click()
      await page.screenshot({ path: `${output}/group-${width}.png` })
      await dialog.getByRole('button', { name: '保存分组', exact: true }).click()
      await dialog.waitFor({ state: 'hidden' })
      const group = writes.find(item => item.path.endsWith('/account-groups/create')).body
      assert.equal(group.fastMode, 'ultrafast')
      assert.equal(group.allowSubagents, false)
      assert.equal(group.modelAccess.mode, 'allowlist')
      assert.deepEqual(group.modelAccess.models, ['gpt-image-2', 'gpt-image-1.5'])
      assert.equal(new Set(group.modelAccess.models).size, group.modelAccess.models.length)
      await page.goto(`${base}/settings`)
      const country = page.getByRole('combobox', { name: '发送策略' })
      await country.click()
      assert.deepEqual(await page.getByRole('option').filter({ visible: true }).allTextContents(), ['跟随位置', '空值', '透传', '自定义'])
      await page.getByRole('option', { name: '自定义', exact: true }).click()
      await page.getByRole('textbox', { name: '自定义国家代码', exact: true }).and(page.locator(':enabled')).fill('JP')
      await page.screenshot({ path: `${output}/country-${width}.png` })
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false)
      assert.deepEqual(errors, [])
      await page.close()
    }
    process.stdout.write('PASS: batch patch, statistics/asOf, reference picker/alignment, group policies/image shortcuts, country settings at desktop/mobile widths\n')
  }
  finally {
    await browser.close()
    await server.close()
  }
}
main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
