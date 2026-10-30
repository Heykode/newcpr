import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'
import { fixtures, isolateNetwork } from './usage-columns.mjs'

async function main() {
  const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href)
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-usage-metrics-qa'
  await mkdir(output, { recursive: true })
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    cacheDir: `${output}/vite-cache`,
    server: { host: '127.0.0.1', port: 0, hmr: false, ws: false },
    plugins: [{
      name: 'isolated-metrics',
      configResolved(config) { config.server.proxy = {} },
      resolveId(id) { return id === '/src/metric-qa.js' ? '\0metric-qa' : undefined },
      load(id) {
        return id === '\0metric-qa'
          ? 'import {getInstanceByDom} from \'echarts/core\'; window.metricCharts=()=>Array.from(document.querySelectorAll(\'[_echarts_instance_]\'),el=>getInstanceByDom(el)?.getOption())'
          : undefined
      },
    }],
  })
  let browser
  try {
    await server.listen()
    const base = new URL(`http://127.0.0.1:${server.httpServer.address().port}`)
    browser = await chromium.launch({ headless: true, args: ['--disable-background-networking', '--no-proxy-server'] })
    for (const width of [1440, 390, 320]) {
      const context = await browser.newContext({ viewport: { width, height: 1000 }, reducedMotion: 'reduce', serviceWorkers: 'block' })
      const report = { blocked: [], api: [] }
      await isolateNetwork(context, base, report)
      await context.routeWebSocket(url => url.hostname === base.hostname && url.port === '0', socket => socket.close())
      const page = await context.newPage()
      const errors = []
      page.on('pageerror', error => errors.push(error.message))
      const calls = []
      let releaseHealth
      const healthGate = new Promise((resolve) => {
        releaseHealth = resolve
      })
      let failPerformance = false
      await page.route('**/api/admin/usage/insights/*', async (route) => {
        const url = new URL(route.request().url())
        const kind = url.pathname.split('/').at(-1)
        if (kind === 'diagnostics')
          return route.fallback()
        const metric = url.searchParams.get('metric') || 'health'
        calls.push(metric)
        if (metric === 'health') {
          await healthGate
          return route.fulfill({ json: { code: 200, message: 'ok', data: fixtures().get('/api/admin/usage/insights/health') } })
        }
        if (kind === 'performance' && failPerformance) {
          return route.fulfill({ status: 503, json: { code: 503, message: '模拟性能卡失败', data: null } })
        }
        if (metric === 'scheduling') {
          const data = fixtures().get('/api/admin/usage/insights/overview').performance
          data.points = [0, 1].map(index => ({ ...data, points: undefined, bucket: `2026-10-30T0${index}:00:00Z`, label: `0${index}:00`, admissionDecisionP95Ms: index === 0 ? 7 : null }))
          return route.fulfill({ json: { code: 200, message: 'ok', data: { metric, granularity: '1h', data } } })
        }
        let summary, value
        if (kind === 'cost') {
          summary = { estimatedCost: '1.25', cacheSavings: '0.25', tierPremium: '0', costPerSuccessfulRequest: '0.5', coverage: { known: 2, partial: 0, unknown: 0, notBillable: 0 } }
          value = metric === 'cache'
            ? { inputTokens: 100, cachedTokenRate: 0.5, cacheHitRequestRate: 0.5 }
            : metric === 'savings' ? '0.25' : { estimatedCost: '1.25', noCacheCost: '1.5' }
        }
        else {
          summary = metric === 'load'
            ? { averageConcurrency: 2, peakConcurrency: 3, admissionDecisionP95Ms: 4, accountSelectionWaitP95Ms: 5 }
            : metric === 'throughput' ? { p10: 10, p50: 20, p90: 30 } : { p50Ms: 10, p95Ms: 20, p99Ms: 30 }
          value = summary
        }
        const points = [0, 1].map(index => ({ bucket: `2026-10-30T0${index}:00:00Z`, label: `0${index}:00`, requestCount: 2, value }))
        return route.fulfill({ json: { code: 200, message: 'ok', data: { metric, granularity: '1h', data: { summary, points } } } })
      })
      try {
        await page.goto(new URL('/usage', base).href)
        const performance = page.getByRole('article').filter({ has: page.getByRole('heading', { name: '响应性能', exact: true }) })
        const cost = page.getByRole('article').filter({ has: page.getByRole('heading', { name: '成本效率', exact: true }) })
        await performance.locator('canvas').first().waitFor()
        await cost.locator('canvas').first().waitFor()
        await page.getByText('正在加载请求健康数据', { exact: true }).waitFor()
        assert(!calls.includes('load'), 'initial request must not calculate concurrency')
        releaseHealth()
        const counts = () => ({ health: calls.filter(x => x === 'health').length, cost: calls.filter(x => ['cost', 'savings', 'cache'].includes(x)).length })
        const before = counts()
        await Promise.all([
          page.waitForResponse(response => response.url().includes('metric=load')),
          performance.getByRole('radio', { name: '负载', exact: true }).click(),
        ])
        await performance.locator('canvas').first().waitFor()
        assert.deepEqual(counts(), before, 'metric change does not reload other cards')
        await page.addScriptTag({ type: 'module', url: new URL('/src/metric-qa.js', base).href })
        await performance.getByRole('radio', { name: '调度', exact: true }).click()
        await page.waitForFunction(() => window.metricCharts?.().some(chart => chart?.series?.some(series => series.name === '准入 P95' && series.data[0] === 7 && series.data[1] === null)))
        failPerformance = true
        await performance.getByRole('radio', { name: 'TTFT', exact: true }).click()
        await performance.getByText('模拟性能卡失败', { exact: true }).waitFor()
        assert(await cost.locator('canvas').count() > 0)
        failPerformance = false
        await performance.getByRole('radio', { name: '输出速率', exact: true }).click()
        await performance.locator('canvas').first().waitFor()
        await cost.getByRole('radio', { name: '缓存', exact: true }).click()
        await cost.locator('canvas').first().waitFor()
        await page.evaluate(() => document.fonts.ready)
        await page.screenshot({ path: `${output}/metrics-${width}.png`, fullPage: true })
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, 'no page overflow')
        assert.deepEqual(errors, [])
        assert.deepEqual(report.blocked, [])
        process.stdout.write(`PASS ${width}px independent cards, selected metrics, failed-card isolation, layout\n`)
      }
      catch (error) {
        await page.screenshot({ path: `${output}/failed-${width}.png`, fullPage: true })
        console.error({ calls, errors, headings: await page.getByRole('heading').allTextContents(), text: (await page.locator('body').textContent()).slice(0, 3500) })
        throw error
      }
      finally {
        releaseHealth()
        await context.close()
      }
    }
  }
  finally {
    await browser?.close()
    await server.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
