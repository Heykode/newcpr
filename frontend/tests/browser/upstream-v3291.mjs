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
import Filters from '/src/views/usage/components/UsageQueryFilters.vue'
import Concurrency from '/src/views/accounts/components/AccountConcurrencyModal.vue'
import Batch from '/src/views/proxies/components/ProxyBatchCreateModal.vue'
import AutoReset from '/src/views/accounts/components/AccountQuotaPanel/AutoReset.vue'
import { useThemeStore } from '/src/stores/modules/theme.ts'
import '@fontsource-variable/inter'
import '@fontsource-variable/jetbrains-mono'
import '/src/styles/index.css'
const mode = ref('dates'), draft = ref({}), open = ref(true)
window.qa = { mode, draft, open }
const pinia = createPinia()
const app = createApp({ setup: () => () => h('main', { style: 'padding:16px;max-width:1100px;margin:auto' }, [
  mode.value === 'dates' ? h(Filters, { range: {}, errors: false, modelValue: draft.value, 'onUpdate:modelValue': v => draft.value = v }) : null,
  mode.value === 'concurrency' ? h(Concurrency, { accountId: 'acct_synthetic', modelValue: open.value, 'onUpdate:modelValue': v => open.value = v }) : null,
  mode.value === 'batch' ? h(Batch, { modelValue: open.value, 'onUpdate:modelValue': v => open.value = v }) : null,
  mode.value === 'auto' ? h(AutoReset, { accountId: 'acct_synthetic', native: true }) : null,
]) }).use(pinia)
useThemeStore(pinia).initializeTheme()
app.mount('#app')
`

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const server = await createServer({ root, server: { host: '127.0.0.1', port: 0 }, plugins: [{
    name: 'v3291-isolated-qa',
    configResolved(config) { config.server.proxy = {} },
    resolveId(id) { return id === '/v3291-qa.js' ? '\0v3291-qa' : undefined },
    load(id) { return id === '\0v3291-qa' ? entry : undefined },
  }] })
  await server.listen()
  const base = `http://127.0.0.1:${server.httpServer.address().port}`
  const output = process.env.QA_OUTPUT_DIR
  if (output)
    await mkdir(output, { recursive: true })
  let browser
  try {
    browser = await chromium.launch({ headless: true })
    for (const width of [1440, 390]) {
      const page = await browser.newPage({ viewport: { width, height: 900 }, timezoneId: 'America/Chicago' })
      const errors = []
      const calls = []
      const unexpected = []
      page.on('pageerror', error => errors.push(error.stack || error.message))
      await page.route('**/*', async (route) => {
        const req = route.request()
        const url = new URL(req.url())
        if (url.origin !== base) {
          unexpected.push(url.href)
          return route.abort()
        }
        if (url.pathname === '/qa')
          return route.fulfill({ contentType: 'text/html', body: '<html><meta name="viewport" content="width=device-width,initial-scale=1"><div id="app"></div><script type="module" src="/v3291-qa.js"></script></html>' })
        url.pathname = url.pathname.replace(/^\/dev\/api\//, '/api/')
        if (!url.pathname.startsWith('/api/'))
          return route.continue()
        const body = req.postDataJSON()
        calls.push({ path: url.pathname, query: url.search, body })
        let data
        if (url.pathname === '/api/admin/accounts/concurrency') {
          data = { total: 1, page: 1, pageSize: 20, items: [{ detail: url.searchParams.get('pool') === 'reserved' ? null : { requestId: 'req_synthetic', clientKeyId: 'key_public', model: 'test-model', clientIp: '127.0.0.1', startedAtMs: Date.now() }, expiresAtMs: Date.now() + 10000 }] }
        }
        else if (url.pathname === '/api/admin/proxies/create-batch') {
          data = [{ status: 'succeeded', data: {} }, { status: 'failed', message: '代理 URL 不合法' }]
        }
        else if (url.pathname.endsWith('/reset-credits/automatic')) {
          data = { accountId: 'acct_synthetic', revision: 1, config: body?.config || { enabled: false, fiveHourUsedMillis: 100000, sevenDayUsedMillis: 100000 }, checkedAt: null, message: null }
        }
        else {
          unexpected.push(url.pathname)
          return route.abort()
        }
        return route.fulfill({ json: { code: 200, message: 'ok', data } })
      })
      await page.goto(`${base}/qa`)
      await page.getByRole('button', { name: '高级筛选', exact: true }).click()
      await page.getByRole('radio', { name: '整天', exact: true }).click()
      for (const [date, hours] of [['2026-03-08', 23], ['2026-11-01', 25]]) {
        await page.getByLabel('开始日期', { exact: true }).fill(date)
        await page.getByLabel('结束日期（含当天）', { exact: true }).fill(date)
        const span = await page.evaluate(() => (Date.parse(window.qa.draft.value.endTime) - Date.parse(window.qa.draft.value.startTime)) / 3600000)
        assert.equal(span, hours, 'date range follows local calendar across DST')
      }
      await page.getByLabel('开始日期', { exact: true }).fill('2026-01-01')
      await page.getByLabel('结束日期（含当天）', { exact: true }).fill('2026-03-01')
      assert.equal(await page.getByLabel('结束日期（含当天）', { exact: true }).inputValue(), '2026-03-01')
      await page.evaluate(() => {
        window.qa.mode.value = 'concurrency'
      })
      await page.getByText('req_synthetic').waitFor({ timeout: 10000 }).catch(async () => {
        throw new Error(JSON.stringify({ body: await page.locator('body').textContent(), calls: calls.map(call => call.path), errors, unexpected }))
      })
      await page.getByRole('radio', { name: 'Guardian 预留' }).click()
      await page.getByText('旧租约（无详情）').waitFor()
      await page.getByRole('radio', { name: '质量检测' }).click()
      await page.getByText('req_synthetic').waitFor()
      assert.ok(calls.some(call => call.query.includes('pool=quality')))
      if (output)
        await page.screenshot({ path: join(output, `concurrency-${width}.png`), fullPage: true })
      await page.evaluate(() => {
        window.qa.mode.value = 'batch'
        window.qa.open.value = true
      })
      await page.getByLabel('代理地址，每行一条', { exact: true }).fill('http://user:synthetic@proxy.example:80\ninvalid')
      await page.getByRole('button', { name: '创建代理', exact: true }).click()
      await page.getByText('第 1 条：已创建', { exact: true }).waitFor()
      await page.getByText('第 2 条：代理 URL 不合法', { exact: true }).waitFor()
      assert.equal(await page.getByLabel('代理地址，每行一条', { exact: true }).inputValue(), '')
      assert.equal(await page.getByRole('button', { name: '创建代理', exact: true }).isDisabled(), true)
      assert.equal(calls.filter(call => call.path.endsWith('/create-batch')).length, 1)
      if (output)
        await page.screenshot({ path: join(output, `proxy-batch-${width}.png`), fullPage: true })
      await page.evaluate(() => {
        window.qa.mode.value = 'auto'
      })
      await page.getByRole('switch', { name: '到期前自动使用', exact: true }).focus()
      await page.keyboard.press('Space')
      await page.getByRole('switch', { name: '限制消费时的周剩余额度', exact: true }).focus()
      await page.keyboard.press('Space')
      await page.getByRole('button', { name: '保存自动重置' }).click()
      await page.getByText('设置已保存；已发送的重置请求不受关闭操作影响。').waitFor()
      const saved = calls.findLast(call => call.body?.config)
      assert.equal(saved.body.config.expiryLeadMinutes, 10)
      assert.equal(saved.body.config.expiryEnabled, true)
      assert.equal(saved.body.config.maxWeeklyRemainingPercent, 0)
      if (output)
        await page.screenshot({ path: join(output, `auto-reset-${width}.png`), fullPage: true })
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
      assert.deepEqual(errors, [])
      assert.deepEqual(unexpected, [])
      await page.close()
    }
    process.stdout.write('Desktop/mobile date DST, concurrency pools, batch outcomes and reset policy passed.\n')
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
