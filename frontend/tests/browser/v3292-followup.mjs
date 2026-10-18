import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'

const root = fileURLToPath(new URL('../..', import.meta.url))
const entry = `
import { createApp, h, computed } from 'vue'
import { createPinia } from 'pinia'
import Pagination from '/src/components/base/BaseTable/BaseTablePagination.vue'
import MetricCard from '/src/views/dashboard/components/MetricCard.vue'
import { usePagedQuery } from '/src/composables/usePagedQuery.ts'
import { dashboardSnapshotView } from '/src/views/dashboard/composables/useDashboard.ts'
import { useThemeStore } from '/src/stores/modules/theme.ts'
import '/src/styles/index.css'
const pinia = createPinia()
createApp({ setup() {
  const pending = []
  const query = usePagedQuery({ initialPageSize:20, load:(params,options)=>new Promise((resolve,reject)=>pending.push({params,options,resolve,reject})) })
  const pagination = computed(()=>({currentPage:query.page.value,pageSize:query.pageSize.value,total:query.total.value}))
  const cards = { credentials:{total:'1',totalValue:1,available:'1',unavailable:'0'},traffic:{todayRequests:'5',todayRequestsValue:5,yesterdayRequestsValue:3,totalRequests:'8'},tokens:{todayTokens:'200',todayTokensValue:200,yesterdayTokensValue:100,totalTokens:'300',totalBillingAmountUsd:'$0.00'},cache:{todayHitRate:'--',todayHitRateValue:null,yesterdayHitRateValue:0.5} }
  const metrics = dashboardSnapshotView({cards,trend:{points:[]},usageRecords:[]}).metrics
  window.qa = {query,pending}
  void query.execute()
  return ()=>h('main',{style:'padding:12px;max-width:1100px;margin:auto'},[
    h('p',{'data-testid':'rows'},query.items.value.map(item=>item.label).join(',')),
    h('p',{'data-testid':'error'},query.error.value),
    h(Pagination,{pagination:pagination.value,loading:query.loading.value,onPageChange:page=>query.execute({page}),onPageSizeChange:pageSize=>query.execute({page:1,pageSize})}),
    h('div',{style:'display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,220px),1fr));gap:12px'},metrics.map(metric=>h(MetricCard,{metric}))),
  ])
}}).use(pinia).mount('#app')
useThemeStore(pinia).initializeTheme()
`

async function main() {
  const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href)
  const server = await createServer({ root, server: { host: '127.0.0.1', port: 0 }, plugins: [{
    name: 'v3292-fixture',
    configResolved(config) { config.server.proxy = {} },
    resolveId(id) { return id === '/followup-qa.js' ? '\0followup-qa' : undefined },
    load(id) { return id === '\0followup-qa' ? entry : undefined },
  }] })
  await server.listen()
  const base = `http://127.0.0.1:${server.httpServer.address().port}`
  let browser
  try {
    browser = await chromium.launch({ headless: true })
    await mkdir(process.env.QA_OUTPUT_DIR, { recursive: true })
    for (const width of [1440, 390, 320]) {
      const page = await browser.newPage({ viewport: { width, height: 900 } })
      const errors = []
      page.on('pageerror', error => errors.push(error.message))
      await page.route('**/*', (route) => {
        const url = new URL(route.request().url())
        assert.equal(url.origin, base)
        if (url.pathname === '/qa')
          return route.fulfill({ contentType: 'text/html', body: '<html><meta name="viewport" content="width=device-width,initial-scale=1"><div id="app"></div><script type="module" src="/followup-qa.js"></script></html>' })
        return route.continue()
      })
      await page.goto(`${base}/qa`)
      await page.waitForFunction(() => window.qa?.pending.length === 1)
      await page.evaluate(() => window.qa.pending[0].resolve({ items: [{ label: 'first page' }], page: { page: 1, pageSize: 20, total: 100, totalPages: 5 } }))
      await page.getByText('first page', { exact: true }).waitFor()
      await page.getByRole('button', { name: '下一页', exact: true }).click()
      await page.evaluate(() => window.qa.pending.at(-1).reject(new Error('Synthetic page failure')))
      await page.getByText('Synthetic page failure', { exact: true }).waitFor()
      assert.equal(await page.locator('[aria-current="page"]:visible').textContent(), '1')
      assert.equal(await page.getByTestId('rows').textContent(), 'first page')
      assert.equal(await page.locator('[title="较昨日"]').count(), 2)
      await page.screenshot({ path: join(process.env.QA_OUTPUT_DIR, `followup-${width}.png`), fullPage: true })
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth), false)
      await page.evaluate(() => {
        void window.qa.query.reloadFromStart()
      })
      assert.equal(await page.getByTestId('rows').textContent(), '')
      await page.evaluate(() => window.qa.pending.at(-1).reject(new Error('Synthetic filter failure')))
      await page.getByText('Synthetic filter failure', { exact: true }).waitFor()
      assert.equal(await page.getByTestId('rows').textContent(), '')
      assert.deepEqual(errors, [])
      await page.close()
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
