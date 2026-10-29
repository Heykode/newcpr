import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'
import { accounts } from '../fixtures/relogin-count-data.mjs'

const entry = `
import {createApp,h} from 'vue'
import {createPinia} from 'pinia'
import {getInstanceByDom} from 'echarts/core'
window.qaChart=()=>getInstanceByDom(document.querySelector('[_echarts_instance_]'))?.getOption()
import Performance from '/src/views/usage/components/UsagePerformanceCard.vue'
import '/src/styles/index.css'
import {setLocale} from '/src/i18n/index.ts'
await setLocale('zh-Hans')
const load={averageConcurrency:1.5,peakConcurrency:2,admissionDecisionP95Ms:25,accountSelectionWaitP95Ms:300}
const data={load,admissionDecisionP50Ms:10,admissionDecisionP95Ms:25,accountSelectionWaitP50Ms:100,accountSelectionWaitP95Ms:300,capacityUtilization:0.4,capacityUtilizationP95:0.8,latencyCoverage:1,firstTokenCoverage:1,admissionDecisionCoverage:1,accountSelectionWaitCoverage:1,capacityCoverage:1,latencyP50Ms:1000,latencyP95Ms:1500,latencyP99Ms:2000,firstTokenP50Ms:100,firstTokenP95Ms:200,firstTokenP99Ms:300,outputThroughputP10:10,outputThroughputP50:20,outputThroughputP90:30}
const points=[0,1,2,3].map(index=>({...data,bucket:new Date(Date.UTC(2026,0,1,0,index*15)).toISOString(),label:'00:'+String(index*15).padStart(2,'0'),load:{...load,averageConcurrency:index+1,peakConcurrency:index+2}}))
createApp({render:()=>h('main',{style:'padding:12px;max-width:1100px;margin:auto'},[
h(Performance,{performance:{...data,points},activity:points.map(point=>({bucket:point.bucket,requestCount:2})),loading:false})
])}).use(createPinia()).mount('#app')
`

async function main() {
  const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href)
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    server: { host: '127.0.0.1', port: 0 },
    plugins: [{
      name: 'isolated-native-upstream',
      configResolved(config) { config.server.proxy = {} },
      resolveId(id) { return id === '/load-qa.js' ? '\0load-qa' : undefined },
      load(id) { return id === '\0load-qa' ? entry : undefined },
    }],
  })
  await server.listen()
  const base = `http://127.0.0.1:${server.httpServer.address().port}`
  const output = process.env.QA_OUTPUT_DIR
  await mkdir(output, { recursive: true })
  const browser = await chromium.launch({ headless: true })
  try {
    for (const width of [1440, 390]) {
      let settings = {
        rotationStrategy: 'smart',
        refreshMarginSeconds: 300,
        refreshConcurrency: 2,
        maxConcurrentPerAccount: 3,
        requestIntervalMs: 0,
        usageRetentionDays: 31,
        opsEventRetentionDays: 30,
        auditRetentionDays: 90,
        disableFast: false,
        requestTuning: {},
        modelMappings: {},
      }
      const page = await browser.newPage({ locale: 'zh-CN', viewport: { width, height: 1000 }, reducedMotion: 'reduce' })
      page.setDefaultTimeout(30000)
      const errors = []
      const writes = []
      page.on('pageerror', error => errors.push(error.message))
      await page.route('**/*', async (route) => {
        const request = route.request()
        const url = new URL(request.url())
        assert.equal(url.origin, base, 'fixtures must never reach a real upstream or backend')
        if (url.pathname === '/qa-load')
          return route.fulfill({ contentType: 'text/html', body: '<html><meta name="viewport" content="width=device-width,initial-scale=1"><div id="app"></div><script type="module" src="/load-qa.js"></script></html>' })
        const path = url.pathname.replace(/^\/dev/, '')
        if (!path.startsWith('/api/'))
          return route.continue()
        let data = { items: [], page: { page: 1, pageSize: 20, total: 0, totalPages: 0 } }
        if (path.endsWith('/auth/status')) {
          data = { authenticated: true }
        }
        else if (path.endsWith('/system/version')) {
          data = { version: 'synthetic', buildType: 'test' }
        }
        else if (path.endsWith('/settings')) {
          data = settings
        }
        else if (path.endsWith('/notifications/channels')) {
          data = { smtp: { enabled: false, host: '', port: 587, username: null, fromName: null, fromEmail: null, security: 'starttls', passwordSet: false }, bark: { enabled: false, serverUrl: '', deviceKeySet: false, level: 'active', sound: null, volume: 5, call: false }, lastTest: null, updatedAt: '2026-01-01T00:00:00Z' }
        }
        else if (path.endsWith('/settings/update')) {
          settings = request.postDataJSON()
          writes.push(settings)
          data = settings
        }
        else if (path.endsWith('/accounts')) {
          const protectedAccount = { ...accounts[0], enabled: true, status: 'rate_limited', errorProtection: { until: new Date(Date.now() - 1000).toISOString(), requiresProbe: true, rules: [{ id: 'synthetic', name: 'Synthetic upstream failures' }] } }
          data = { items: [protectedAccount], page: { page: 1, pageSize: 20, total: 1, totalPages: 1 }, summary: { total: 1, rateLimited: 1 } }
        }
        else if (path.endsWith('/accounts/query') || path.endsWith('/reset-credits/batches')) {
          data = []
        }
        return route.fulfill({ json: { code: 200, message: 'ok', data } })
      })
      await page.goto(`${base}/settings`)
      const toggle = page.getByRole('switch', { name: '启用错误保护', exact: true })
      await toggle.waitFor()
      assert.equal(await toggle.isChecked(), false)
      await page.getByRole('button', { name: '添加规则', exact: true }).click()
      await page.getByRole('textbox', { name: '规则名称', exact: true }).fill('Synthetic upstream failures')
      await page.getByRole('textbox', { name: '候选值 1', exact: true }).fill('503')
      await toggle.locator('..').click()
      await page.getByRole('spinbutton', { name: '失败阈值', exact: true }).fill('3')
      const savedResponse = page.waitForResponse(response => response.url().endsWith('/settings/update'))
      await page.getByRole('button', { name: '保存全部设置', exact: true }).click()
      await savedResponse
      assert.equal(writes.length, 1)
      const saved = writes[0].requestTuning.accountErrorProtection
      assert.equal(saved.enabled, true)
      assert.equal(saved.threshold, 3)
      assert.equal(saved.rules.length, 1)
      assert.equal(saved.rules[0].conditions[0].values[0], '503')
      assert.equal(saved.probeModel, null)
      await page.reload()
      await toggle.waitFor()
      assert.equal(await toggle.isChecked(), true)
      await page.getByRole('button', { name: '编辑规则：Synthetic upstream failures', exact: true }).click()
      await page.getByRole('button', { name: '添加候选值', exact: true }).click()
      await page.getByRole('textbox', { name: '候选值 2', exact: true }).fill('502')
      await page.getByRole('button', { name: '删除候选值 2', exact: true }).click()
      assert.equal(await page.getByRole('textbox', { name: '候选值 2', exact: true }).count(), 0)
      assert.equal(await page.getByText('Cannot read properties of undefined', { exact: false }).count(), 0)
      await page.screenshot({ path: `${output}/settings-${width}.png`, fullPage: true })
      await page.locator('section, article').filter({ has: toggle }).last().screenshot({ path: `${output}/protection-${width}.png` })
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false)

      await page.goto(`${base}/accounts`)
      await page.getByText('等待恢复探测', { exact: true }).first().waitFor()
      await page.screenshot({ path: `${output}/accounts-${width}.png`, fullPage: true })
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false)

      await page.goto(`${base}/qa-load`)
      await page.getByRole('radio', { name: '负载', exact: true }).click()
      await page.getByText('平均并发', { exact: true }).first().waitFor()
      await page.waitForFunction(() => window.qaChart()?.series.some(series => series.data.length === 4))
      await page.waitForFunction(() => {
        const canvas = document.querySelector('canvas')
        if (!canvas)
          return false
        const pixels = canvas.getContext('2d').getImageData(0, canvas.height / 3, canvas.width, Math.floor(canvas.height / 2)).data
        let colored = 0
        for (let i = 0; i < pixels.length; i += 4) {
          if (pixels[i + 3] > 80 && Math.max(pixels[i], pixels[i + 1], pixels[i + 2]) - Math.min(pixels[i], pixels[i + 1], pixels[i + 2]) > 60)
            colored++
        }
        return colored > 100
      })
      assert.equal(await page.getByText('Cannot read properties of undefined', { exact: false }).count(), 0)
      assert.ok(await page.locator('canvas').count() > 0)
      // Wait for the component's 240ms ECharts animation before visual evidence.
      await page.waitForTimeout(350)
      await page.screenshot({ path: `${output}/load-${width}.png`, fullPage: true })
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false)
      await page.getByRole('radio', { name: '调度', exact: true }).click()
      await page.getByText('容量覆盖', { exact: true }).waitFor()
      await page.waitForFunction(() => window.qaChart()?.series.some(series => series.name === '容量 P95' && series.data.length === 4))
      await page.waitForTimeout(350)
      await page.screenshot({ path: `${output}/scheduling-${width}.png`, fullPage: true })
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false)
      assert.deepEqual(errors, [])
      await page.close()
      process.stdout.write(`PASS ${width}px: protection settings save/reload, rules, account status, load chart\n`)
    }
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
