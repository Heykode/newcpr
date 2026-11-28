import assert from 'node:assert/strict'
import { mkdir, writeFile } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { isolateNetwork } from './usage-columns.mjs'

async function main() {
  const base = new URL(process.env.QA_BASE_URL || 'http://127.0.0.1:5279')
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-usage-filters-ui'
  const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href)
  const browser = await chromium.launch({ channel: process.env.PLAYWRIGHT_CHANNEL || 'chrome', headless: true })
  const report = { blocked: [], api: [], calls: [], errors: [], widths: [] }
  const accounts = [
    { id: 'acct-filter-a', email: 'filter-a@example.invalid', name: '测试账号 A', customName: '主工作区', deleted: false },
    { id: 'acct-filter-b', email: 'filter-b@example.invalid', name: '历史账号 B', customName: null, deleted: true },
  ]
  const combined = process.env.QA_CAPTURE_OVERLAY === '1'
  await mkdir(output, { recursive: true })
  try {
    for (const width of [1440, 390, 320]) {
      report.calls = []
      const context = await browser.newContext({ viewport: { width, height: 1000 }, timezoneId: 'America/Los_Angeles', serviceWorkers: 'block' })
      await isolateNetwork(context, base, report)
      await context.route('**/*', async (route) => {
        const url = new URL(route.request().url())
        const path = url.pathname.replace(/^\/dev(?=\/api\/)/, '')
        if (url.origin !== base.origin || !path.startsWith('/api/'))
          return route.fallback()
        const query = Object.fromEntries(url.searchParams)
        report.calls.push({ path, query })
        let data
        if (path === '/api/admin/usage/account-options')
          data = accounts.filter(a => !query.search || JSON.stringify(a).includes(query.search))
        else if (path === '/api/admin/client-keys')
          data = { items: [{ id: 'key-filter', name: '测试 Key', prefix: 'sk_fixture' }], nextCursor: null, total: 1 }
        else if (path === '/api/admin/account-groups')
          data = { items: [{ id: 'group-filter', name: '测试分组' }], page: { page: 1, pageSize: 50, total: 1, totalPages: 1 } }
        else if (path === '/api/admin/usage/records/detail')
          data = { id: query.id, requestId: query.id, trace: null, attempts: [], relatedRequests: [], attemptsComplete: true, metadata: {} }
        else if (combined && path === '/api/admin/request-captures/config')
          data = { config: { enabled: false, globalErrors: false, includeMedia: false, quotaMib: 1024, retentionDays: 7 }, globalActive: false, storageFault: false, skipped: 0 }
        else if (combined && path === '/api/admin/request-captures/by-request')
          data = [{ id: `capture-${query.requestId}`, taskId: 'capture-task', requestId: query.requestId, bytes: 100, incomplete: false, createdAt: '2026-09-29T00:00:00Z' }]
        else if (combined && path === '/api/admin/request-captures/records')
          data = { text: `Synthetic capture for ${query.id}`, nextOffset: null }
        else
          return route.fallback()
        return route.fulfill({ json: { code: 200, message: 'ok', data } })
      })
      const page = await context.newPage()
      page.setDefaultTimeout(15_000)
      page.setDefaultNavigationTimeout(15_000)
      page.on('pageerror', error => report.errors.push(error.message))
      await page.goto(`${base}usage?upstreamStatusCode=403`)
      await page.getByLabel('请求搜索', { exact: true }).waitFor()
      const upstream = page.getByRole('combobox', { name: '上游模式', exact: true })
      await upstream.click()
      await Promise.all([
        page.waitForResponse(response => response.url().includes('/usage/records?') && new URL(response.url()).searchParams.get('upstreamMode') === 'excel'),
        page.getByRole('option', { name: 'Excel', exact: true }).click(),
      ])
      await upstream.click()
      await Promise.all([
        page.waitForResponse(response => response.url().includes('/usage/records?') && !new URL(response.url()).searchParams.has('upstreamMode')),
        page.getByRole('option', { name: '全部', exact: true }).click(),
      ])
      const placement = await page.locator('section[aria-label="请求筛选"]').evaluate((filters) => {
        const heading = [...document.querySelectorAll('h2')].find(element => element.textContent.trim() === '请求明细')
        const insights = [...document.querySelectorAll('h3,h2')].find(element => element.textContent.trim() === '热点诊断')
        const card = heading?.closest('section,article')
        return {
          insideDetails: Boolean(card?.contains(filters)),
          belowHeading: Boolean(heading && (heading.compareDocumentPosition(filters) & Node.DOCUMENT_POSITION_FOLLOWING)),
          belowInsights: Boolean(insights && (insights.compareDocumentPosition(filters) & Node.DOCUMENT_POSITION_FOLLOWING)),
          count: document.querySelectorAll('section[aria-label="请求筛选"]').length,
        }
      })
      assert.deepEqual(placement, { insideDetails: false, belowHeading: false, belowInsights: false, count: 1 }, `one shared filter panel precedes insights at ${width}px`)
      await page.getByText('Pro', { exact: true }).first().waitFor()
      await page.getByRole('button', { name: '筛选时间范围', exact: true }).click()
      const rangeDialog = page.getByRole('dialog').filter({ has: page.getByRole('form', { name: '时间范围', exact: true }) })
      await rangeDialog.getByLabel('开始时间', { exact: true }).fill('2026-01-01T00:00')
      assert.ok(!report.calls.some(call => call.query.startTime?.startsWith('2026-01-01')))
      await rangeDialog.getByLabel('结束时间', { exact: true }).fill('2026-02-02T00:00')
      assert.ok(!report.calls.some(call => call.query.startTime?.startsWith('2026-01-01')))
      await Promise.all([
        page.waitForResponse(response => response.url().includes('/usage/records?') && response.url().includes('2026-01-01')),
        rangeDialog.getByRole('button', { name: '应用', exact: true }).click(),
      ])
      const customRange = new URL(page.url()).searchParams
      for (const endpoint of ['/usage/records', '/usage/records/summary', '/usage/insights/health', '/usage/insights/performance', '/usage/insights/cost', '/usage/insights/diagnostics']) {
        await page.waitForTimeout(30)
        assert.ok(report.calls.some(call => call.path.endsWith(endpoint)
          && call.query.startTime === customRange.get('startTime')
          && call.query.endTime === customRange.get('endTime')), `${endpoint} shares the applied range`)
      }
      await page.getByRole('button', { name: '筛选时间范围', exact: true }).click()
      await rangeDialog.getByRole('button', { name: '最近 7 天', exact: true }).click()
      await page.waitForURL(url => url.searchParams.get('timeRange') === '7d' && !url.searchParams.has('startTime'))

      const account = page.getByLabel('搜索账号', { exact: true })
      await account.fill('filter-')
      await page.getByRole('option', { name: /filter-a@example.invalid/ }).click()
      await page.getByRole('option', { name: /filter-b@example.invalid/ }).click()
      await account.press('Escape')
      await page.waitForURL(url => url.searchParams.get('accountIds') === 'acct-filter-a,acct-filter-b')
      const endpoints = ['/usage/records', '/usage/records/summary', '/usage/insights/health', '/usage/insights/performance', '/usage/insights/cost', '/usage/insights/diagnostics']
      await Promise.all([
        ...endpoints.map(endpoint => page.waitForResponse((response) => {
          const url = new URL(response.url())
          return url.pathname.endsWith(endpoint) && url.searchParams.get('search') === 'literal_%' && url.searchParams.get('accountIds') === 'acct-filter-a,acct-filter-b'
        })),
        page.getByLabel('请求搜索', { exact: true }).fill('literal_%'),
      ])
      for (const endpoint of endpoints) {
        assert(report.calls.some(call => call.path.endsWith(endpoint) && call.query.search === 'literal_%' && call.query.accountIds === 'acct-filter-a,acct-filter-b'), `${endpoint} shares filters`)
      }
      await page.getByRole('button', { name: '更多筛选', exact: true }).click()
      await page.getByLabel('搜索API Key', { exact: true }).fill('测试')
      await page.getByRole('button', { name: /测试 Key sk_fixture/ }).click()
      await page.getByLabel('搜索请求分组', { exact: true }).fill('测试')
      await page.getByRole('button', { name: /测试分组 group-filter/ }).click()
      await page.getByRole('radio', { name: '错误排查', exact: true }).click()
      assert.equal(await page.locator('section[aria-label="请求筛选"]').count(), 1)
      await page.getByLabel('客户端状态码', { exact: true }).fill('502')
      assert.equal(await page.getByLabel('上游状态码', { exact: true }).count(), 0)
      await page.getByLabel('已应用的钻取条件').getByText('上游状态码：403', { exact: true }).waitFor()
      await page.waitForResponse(response => response.url().includes('/operations/errors?')
        && response.url().includes('clientStatusCode=502') && response.url().includes('upstreamStatusCode=403'))
      assert.equal(report.calls.filter(call => call.path.endsWith('/usage/records/detail')).length, 0)
      await Promise.all([
        page.waitForResponse(response => response.url().includes('/usage/records/detail')),
        page.getByRole('button', { name: '查看错误详情', exact: true }).first().click(),
      ])
      await page.getByRole('region', { name: '请求诊断', exact: true }).waitFor()
      const detail = report.calls.filter(call => call.path.endsWith('/usage/records/detail')).at(-1)
      assert.equal(detail.query.id, 'req_columns_1')
      if (combined) {
        await page.getByText('Synthetic capture for capture-req_columns_1', { exact: true }).waitFor()
        assert.equal(report.calls.filter(call => call.path.endsWith('/request-captures/by-request')).at(-1).query.requestId, detail.query.id)
      }
      await page.getByRole('button', { name: '关闭', exact: true }).first().click()
      await page.locator('[role="dialog"]').waitFor({ state: 'detached' })
      if (combined)
        await page.getByRole('switch', { name: '详细错误采集（全局）', exact: true }).waitFor()
      await page.getByRole('button', { name: '更多筛选', exact: true }).scrollIntoViewIfNeeded()
      const fields = await page.locator('section[aria-label="请求筛选"] input').evaluateAll(inputs => inputs.map((input) => {
        const box = input.getBoundingClientRect()
        return { left: box.left, right: box.right, width: box.width }
      }))
      assert(fields.every(box => box.left >= 0 && box.right <= width + 1 && box.width > 0), `fields fit ${width}`)
      await page.screenshot({ path: `${output}/filters-${width}.png`, fullPage: true })
      await page.getByRole('button', { name: '更多筛选', exact: true }).click()
      const savedRange = new URL(page.url()).searchParams.get('timeRange')
      await page.getByRole('button', { name: '清除客户端状态码条件', exact: true }).click()
      await page.waitForURL(url => !url.searchParams.has('clientStatusCode'))
      assert.equal(new URL(page.url()).searchParams.get('timeRange'), savedRange, 'individual removal retains the time range')
      assert.equal(new URL(page.url()).searchParams.get('accountIds'), 'acct-filter-a,acct-filter-b')
      await page.getByRole('button', { name: '更多筛选', exact: true }).click()
      await page.getByLabel('客户端状态码', { exact: true }).fill('502')
      await page.waitForURL(url => url.searchParams.get('clientStatusCode') === '502')
      const before = new URL(page.url()).searchParams
      await page.reload()
      await page.getByRole('button', { name: '更多筛选', exact: true }).click()
      assert.equal(await page.getByLabel('客户端状态码', { exact: true }).inputValue(), '502')
      assert.equal(new URL(page.url()).searchParams.get('accountIds'), before.get('accountIds'))
      await page.getByRole('button', { name: '移除上游状态码筛选', exact: true }).click()
      await page.waitForURL(url => !url.searchParams.has('upstreamStatusCode'))
      await page.getByRole('button', { name: '重置全部筛选', exact: true }).click()
      await page.waitForURL(url => !url.searchParams.has('accountIds'))
      assert.equal(new URL(page.url()).searchParams.get('view'), 'errors')
      report.widths.push(width)
      await context.close()
    }
    assert.deepEqual(report.errors, [])
    assert.deepEqual(report.blocked, [])
    process.stdout.write(`${JSON.stringify({ widths: report.widths, requests: report.calls.length, combined, passed: true })}\n`)
  }
  finally {
    await writeFile(`${output}/report.json`, JSON.stringify(report, null, 2))
    await browser.close()
  }
}
main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
