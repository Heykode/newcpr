import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'
import { isolateNetwork } from './usage-columns.mjs'

const entry = `
import { createApp, h, ref } from 'vue'
import { createPinia } from 'pinia'
import { i18n, setLocale, t } from '/src/i18n/index.ts'
import Menu from '/src/components/AppLanguageMenu.vue'
import ImportModal from '/src/components/ClientImportModal.vue'
import Credits from '/src/views/accounts/components/AccountQuotaPanel/Credits.vue'
import Preview from '/src/views/theme/components/ThemePreviewCanvas.vue'
import Dashboard from '/src/views/theme/components/ThemeDashboardPreview.vue'
import { useThemeStore } from '/src/stores/modules/theme.ts'
import '/src/styles/index.css'
await setLocale('zh-Hans')
const open = ref(false), selected = ref(0), left = ref(0)
window.qa = { open, selected, left }
const pinia = createPinia()
const app = createApp({ setup: () => () => h('main', { style:'padding:12px;min-width:0' }, [
 h(Menu), h('h1', t('usage.page.requestDetails')),
 h(Credits, {credits:{hasCredits:true,unlimited:false,balance:'12345678901234567890.125'}}),
 h(ImportModal, {modelValue:open.value, 'onUpdate:modelValue':v=>open.value=v, onSelect:()=>selected.value++, onAfterLeave:()=>left.value++}),
 h('div', {style:'height:600px;margin-top:16px'}, [h(Preview, {theme:'light',style:{}, class:'h-full', 'data-testid':'preview'}, {default:()=>h(Dashboard)})]),
]) }).use(pinia).use(i18n)
useThemeStore(pinia).initializeTheme()
app.mount('#app')
`

async function main() {
  const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href)
  const root = fileURLToPath(new URL('../..', import.meta.url))
  const output = process.env.QA_OUTPUT_DIR
  await mkdir(output, { recursive: true })
  const server = await createServer({ root, server: { host: '127.0.0.1', port: 5198, strictPort: true }, plugins: [{
    name: 'v3302-qa',
    configResolved(config) { config.server.proxy = {} },
    resolveId(id) { return id === '/v3302-qa.js' ? '\0v3302-qa' : undefined },
    load(id) { return id === '\0v3302-qa' ? entry : undefined },
  }] })
  let browser
  try {
    await server.listen()
    const base = `http://127.0.0.1:${server.httpServer.address().port}`
    browser = await chromium.launch({ headless: true })
    for (const width of [1440, 390, 320]) {
      const page = await browser.newPage({ locale: 'zh-CN', viewport: { width, height: 1000 }, reducedMotion: 'reduce' })
      page.setDefaultTimeout(30000)
      const errors = []
      page.on('pageerror', error => errors.push(error.message))
      await page.route('**/*', (route) => {
        const url = new URL(route.request().url())
        if (url.origin !== base)
          throw new Error(`External browser request: ${url.origin}`)
        if (url.pathname === '/qa')
          return route.fulfill({ contentType: 'text/html', body: '<html><meta name="viewport" content="width=device-width,initial-scale=1"><div id="app"></div><script type="module" src="/v3302-qa.js"></script></html>' })
        if (url.pathname.startsWith('/dev/api/'))
          return route.fulfill({ json: { code: 200, message: 'ok', data: { authenticated: false } } })
        return route.continue()
      })
      try {
        await page.goto(`${base}/qa`)
        await page.getByRole('heading', { name: '请求明细', exact: true }).waitFor()
        for (const language of ['en', 'zh-Hant', 'zh-Hans']) {
          await page.getByRole('combobox').selectOption(language)
          await page.waitForFunction(value => document.documentElement.lang === value, language)
          assert.equal(await page.evaluate(() => localStorage.getItem('codex-proxy-rs-locale')), language)
          assert.ok(!(await page.locator('h1').first().textContent()).includes('usage.page.'))
          await page.evaluate(() => window.qa.open.value = true)
          const dialog = page.getByRole('dialog')
          await dialog.waitFor()
          const bounds = await dialog.boundingBox()
          assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= width + 1)
          assert.equal(await dialog.evaluate(el => el.scrollWidth <= el.clientWidth + 1), true)
          await page.screenshot({ path: `${output}/import-${language}-${width}.png` })
          const selected = await page.evaluate(() => window.qa.selected.value)
          await dialog.getByRole('button', { name: /CC Switch/ }).click()
          await dialog.waitFor({ state: 'hidden' })
          assert.equal(await page.evaluate(() => window.qa.selected.value), selected + 1)
        }
        await page.getByText('12,345,678,901,234,567,890.125', { exact: true }).waitFor()
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), true)
        await page.screenshot({ path: `${output}/theme-${width}.png`, fullPage: true })
        await page.goto(`${base}/login`)
        await page.getByRole('combobox').selectOption('en')
        await page.waitForFunction(() => document.documentElement.lang === 'en')
        await page.reload()
        await page.getByRole('combobox').waitFor()
        assert.equal(await page.locator('html').getAttribute('lang'), 'en')
        await page.unrouteAll()
        const report = { blocked: [], api: [] }
        await isolateNetwork(page.context(), new URL(base), report)
        await page.evaluate(() => localStorage.setItem('codex-proxy-rs-locale', 'zh-Hans'))
        await page.goto(`${base}/usage`)
        await page.getByLabel('请求搜索', { exact: true }).waitFor()
        const placement = await page.locator('section[aria-label="请求筛选"]').evaluate((filters) => {
          const heading = [...document.querySelectorAll('h2')].find(el => el.textContent.trim() === '请求明细')
          return Boolean(heading?.closest('section,article')?.contains(filters)
            && (heading.compareDocumentPosition(filters) & Node.DOCUMENT_POSITION_FOLLOWING))
        })
        assert.equal(placement, true, 'usage filters stay inside request details')
        assert.deepEqual(report.blocked, [])
        await page.screenshot({ path: `${output}/usage-${width}.png`, fullPage: true })
        assert.deepEqual(errors, [])
        process.stdout.write(`PASS ${width}px: three languages, import, credit balance, theme, persisted login locale and usage filter placement\n`)
      }
      catch (error) {
        await page.screenshot({ path: `${output}/failure-${width}.png`, fullPage: true })
        console.error(errors)
        throw error
      }
      finally {
        await page.close()
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
