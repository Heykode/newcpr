import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'

const entry = `
import {createApp,h} from 'vue'
import {i18n,setLocale} from '/src/i18n'
import Dashboard from '/src/views/dashboard/components/AccountOverviewCard.vue'
import Quota from '/src/views/accounts/components/AccountQuotaSummaryCell/index.vue'
import {mobileAccounts} from '/tests/fixtures/mobile-account-data.mjs'
import '/src/styles/index.css'
await setLocale('zh-Hans')
const row = structuredClone(mobileAccounts[0])
row.usage.requestCount = 0
const account = {
  id:'synthetic',email:'snapshot@example.invalid',provider:'openai',authenticationKind:'oauth',
  metric:'tokens',metricValue:'0',metricValueRaw:0,lastUsedRelative:null,lastUsed:'从未',
  usageWindow:null
}
createApp({render:()=>h('main',{class:'grid gap-6 p-3'},[
  h('div',{id:'missing'},[h(Dashboard,{accounts:[account]})]),
  h('div',{id:'known'},[h(Dashboard,{accounts:[{...account,usageWindow:row.quota.windows[0]}]})]),
  h('div',{id:'quota',class:'max-w-96'},[h(Quota,{account:row})])
])}).use(i18n).mount('#app')
`

async function main() {
  const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href)
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-post-v3360-ui'
  await mkdir(output, { recursive: true })
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    cacheDir: `${output}/vite-cache`,
    server: { host: '127.0.0.1', port: 0, hmr: false, ws: false },
    plugins: [{
      name: 'post-v3360-isolated-preview',
      configResolved(config) { config.server.proxy = {} },
      transformIndexHtml() { return '<html><body><div id="app"></div><script type="module" src="/post-v3360-qa.js"></script></body></html>' },
      resolveId(id) { return id === '/post-v3360-qa.js' ? '\0post-v3360-qa' : undefined },
      load(id) { return id === '\0post-v3360-qa' ? entry : undefined },
    }],
  })
  let browser
  try {
    await server.listen()
    const base = `http://127.0.0.1:${server.httpServer.address().port}`
    browser = await chromium.launch({ headless: true, args: ['--no-proxy-server'] })
    for (const width of [1440, 390, 320]) {
      const page = await browser.newPage({ viewport: { width, height: 1000 }, reducedMotion: 'reduce', serviceWorkers: 'block' })
      const errors = []
      page.on('pageerror', error => errors.push(error.message))
      await page.route('**/*', route => route.request().url().startsWith(base)
        ? route.continue()
        : route.abort())
      await page.goto(base)
      try {
        await page.locator('#known [role="progressbar"]').waitFor()
      }
      catch (error) {
        console.error({ errors, text: await page.locator('body').textContent() })
        await page.screenshot({ path: `${output}/failed-${width}.png`, fullPage: true })
        throw error
      }
      assert.equal(await page.locator('#missing [role="progressbar"]').count(), 0)
      assert.equal(await page.locator('#missing').getByText('额度待观测', { exact: true }).count(), 0)
      assert.ok(await page.locator('#quota [role="progressbar"]').count() > 0)
      assert.equal(await page.locator('#quota').getByText('Tokens', { exact: true }).count(), 0)
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false)
      assert.deepEqual(errors, [])
      await page.screenshot({ path: `${output}/quota-${width}.png`, fullPage: true })
      await page.close()
      process.stdout.write(`PASS ${width}px: missing window hidden, native quota visible with zero requests\n`)
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
