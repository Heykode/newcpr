import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'

const root = fileURLToPath(new URL('../..', import.meta.url))
const entry = `
import { createApp, h, ref } from 'vue'
import { createPinia } from 'pinia'
import Concurrency from '/src/views/accounts/components/AccountConcurrencyModal.vue'
import ErrorDetail from '/src/views/usage/components/UsageErrorDetailModal.vue'
import FullDetail from '/src/views/usage/components/UsageRecordDetailModal.vue'
import Records from '/src/views/usage/components/UsageRecordsTable.vue'
import { useThemeStore } from '/src/stores/modules/theme.ts'
import '/src/styles/index.css'
const mode = ref('table'), open = ref(false), full = ref(false)
const record = ref({
  id: 'failed-request', requestId: 'failed-request', logicalOutcome: 'failed', message: 'Synthetic final failure',
  accountName: 'Synthetic account', route: '/v1/responses', requestedModel: 'test-model',
  upstreamModel: 'test-model', clientTransport: 'http_sse', upstreamTransport: 'websocket',
  statusCode: 502, clientStatusCode: 502, upstreamStatusCode: 429,
  createdAtDisplay: '2026-10-18 12:00', latencyMsDisplay: '2 秒',
  firstTokenLatencyMsDisplay: '1 秒', attemptCount: 1, tokenDetails: {}, costs: [], providerMetadata: {},
  costCoverage: {}, attemptsComplete: false,
})
const pinia = createPinia()
window.qa = { mode, open, full, record }
const app = createApp({ setup: () => () => h('main', { style: 'padding:12px' }, [
  mode.value === 'table' ? h(Records, {
    columns: [{key:'outcome', label:'状态', kind:'custom'}, {key:'errorMessage',label:'最终错误',kind:'custom'}],
    rows: [{id:'a',outcome:'failed',errorMessage:'Final failure summary'}, {id:'b',outcome:'succeeded',errorMessage:'Earlier failure must stay hidden'}, {id:'c',outcome:'incomplete',errorCode:'incomplete_code'}],
  }) : null,
  mode.value === 'error' ? h(full.value ? FullDetail : ErrorDetail, {record:record.value, modelValue:open.value, 'onUpdate:modelValue':v=>open.value=v, onFullDetail:()=>full.value=true}) : null,
  mode.value === 'concurrency' ? h(Concurrency, {accountId:'synthetic', modelValue:open.value,'onUpdate:modelValue':v=>open.value=v}) : null,
]) }).use(pinia)
useThemeStore(pinia).initializeTheme()
app.mount('#app')
`

async function main() {
  const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href)
  const server = await createServer({ root, server: { host: '127.0.0.1', port: 0 }, plugins: [{
    name: 'post-v3291-qa',
    configResolved(config) { config.server.proxy = {} },
    resolveId(id) { return id === '/post-qa.js' ? '\0post-qa' : undefined },
    load(id) { return id === '\0post-qa' ? entry : undefined },
  }] })
  await server.listen()
  const base = `http://127.0.0.1:${server.httpServer.address().port}`
  const output = process.env.QA_OUTPUT_DIR
  if (output)
    await mkdir(output, { recursive: true })
  let browser
  try {
    browser = await chromium.launch({ headless: true })
    for (const width of [1440, 390, 320]) {
      const page = await browser.newPage({ viewport: { width, height: 900 } })
      const errors = []
      const calls = []
      page.on('pageerror', error => errors.push(error.message))
      await page.route('**/*', async (route) => {
        const url = new URL(route.request().url())
        assert.equal(url.origin, base)
        if (url.pathname === '/qa')
          return route.fulfill({ contentType: 'text/html', body: '<html><meta name="viewport" content="width=device-width,initial-scale=1"><div id="app"></div><script type="module" src="/post-qa.js"></script></html>' })
        const path = url.pathname.replace(/^\/dev\/api\//, '/api/')
        if (!path.startsWith('/api/'))
          return route.continue()
        calls.push(path)
        let data
        if (path === '/api/admin/accounts/concurrency') {
          const pageNumber = Number(url.searchParams.get('page'))
          const ids = pageNumber === 1 ? Array.from({ length: 100 }, (_, i) => i) : [99, 100]
          data = { page: pageNumber, pageSize: 100, total: 101, items: ids.map(id => ({ leaseId: `lease-${id}`, expiresAtMs: Date.now() + 30000, detail: { requestId: `request-${id}`, clientKeyId: 'public-key-id', model: 'test-model', startedAtMs: Date.now() } })) }
        }
        else if (path === '/api/admin/usage/records/detail') {
          data = { requestId: 'failed-request', trace: null, attemptsComplete: false, relatedRequests: [] }
        }
        else if (path === '/api/admin/request-captures/by-request') {
          data = []
        }
        else {
          throw new Error(`Unexpected API request: ${path}`)
        }
        return route.fulfill({ json: { code: 200, message: 'ok', data } })
      })
      await page.goto(`${base}/qa`)
      await page.getByText('Final failure summary', { exact: true }).waitFor()
      await page.getByText('incomplete_code', { exact: true }).waitFor()
      assert.equal(await page.getByText('Earlier failure must stay hidden', { exact: true }).count(), 0)
      assert.equal(calls.length, 0, 'list never fetches diagnostic material')
      await page.evaluate(() => {
        window.qa.mode.value = 'error'
      })
      await page.waitForTimeout(100)
      assert.equal(calls.length, 0, 'closed error dialog must not fetch diagnostics')
      await page.evaluate(() => {
        window.qa.open.value = true
      })
      await page.getByRole('dialog', { name: '错误详情', exact: true }).waitFor()
      await page.getByText('Synthetic final failure', { exact: true }).waitFor()
      await page.getByText('尝试记录可能不完整', { exact: true }).waitFor()
      if (output)
        await page.screenshot({ path: join(output, `error-${width}.png`), fullPage: true })
      await page.getByRole('button', { name: '完整详情', exact: true }).click()
      await page.getByRole('dialog', { name: '使用记录详情', exact: true }).waitFor()
      assert.ok(calls.includes('/api/admin/usage/records/detail'))
      await page.evaluate(() => {
        window.qa.mode.value = 'concurrency'
        window.qa.open.value = true
      })
      await page.getByText('request-100').waitFor().catch(async () => {
        throw new Error(JSON.stringify({ body: await page.locator('body').textContent(), calls, errors }))
      })
      assert.equal(await page.locator('tbody tr').count(), 101)
      const panel = page.locator('[aria-busy]')
      const height = await panel.evaluate(element => ({ height: element.clientHeight, scroll: element.scrollHeight }))
      assert.ok(height.scroll > height.height)
      assert.ok(height.height <= 540)
      if (output)
        await page.screenshot({ path: join(output, `post-concurrency-${width}.png`), fullPage: true })
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
      assert.deepEqual(errors, [])
      await page.close()
    }
    process.stdout.write('Error summary/detail, lazy diagnostics and scrolling concurrency passed at 1440/390/320px.\n')
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
