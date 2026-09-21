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
import UsageTable from '/src/views/usage/components/UsageRecordsTable.vue'
import ForecastCapacity from '/src/views/accounts/components/AccountQuotaForecastModal/ForecastCapacity.vue'
import UpdateModal from '/src/layout/components/SystemUpdateModal/index.vue'
import { useSystemUpdateStore } from '/src/stores/modules/system-update.ts'
import { useThemeStore } from '/src/stores/modules/theme.ts'
import '@fontsource-variable/inter'
import '@fontsource-variable/jetbrains-mono'
import '/src/styles/index.css'
const pinia = createPinia()
const open = ref(false)
const notes = 'Synthetic current note: ' + 'long note '.repeat(30)
const app = createApp({
  setup() {
    const store = useSystemUpdateStore(pinia)
    window.qa = { store, open }
    return () => h('main', { style: 'padding:16px;max-width:1100px;margin:auto' }, [
      h('h1', { style: 'font-size:20px;margin-bottom:16px' }, 'Stable update verification'),
      h(UsageTable, { columns: [
        { key: 'accountEmail', label: 'Account', width: 240 },
        { key: 'model', label: 'Model', width: 200 },
        { key: 'clientApiKeyName', label: 'Key', width: 144 },
        { key: 'billing', label: 'Billing', width: 144 }
      ], rows: [{
        id: 'synthetic-usage', accountEmail: 'synthetic@example.invalid',
        accountCustomName: notes, model: 'synthetic-model',
        clientApiKeyName: 'Synthetic Key ' + 'long name '.repeat(20),
        billing: { longContextBillingApplied: true, totalAmountDisplay: '$1.23',
          inputAmountDisplay:'$1.00',outputAmountDisplay:'$0.23',cacheReadAmountDisplay:'$0.00',
          cacheWriteAmountDisplay:'$0.00',inputPriceDisplay:'$2.00',outputPriceDisplay:'$4.00',
          cacheWritePriceDisplay:'$0.00',serviceTierDisplay:'Standard',multiplierDisplay:'1.00x',
          standardAmountDisplay:'$1.23' }
      }] }),
      h('div', { style: 'max-width:480px;margin-top:20px' }, [
        h(ForecastCapacity, { forecast: {
          source: { usedPercent: 30, usedPercentDisplay: '30%', tokensDisplay: '3,000', usdDisplay: '$3.00' },
          estimatedTokensDisplay: '10,000', estimatedUsdDisplay: '$10.00',
          remainingTokensDisplay: '7,000', remainingUsdDisplay: '$7.00',
          targetDays: 7, extrapolated: false, lowSample: false
        } })
      ]),
      h(UpdateModal, { modelValue: open.value, 'onUpdate:modelValue': value => open.value = value })
    ])
  }
})
app.use(pinia)
useThemeStore(pinia).initializeTheme()
app.mount('#app')
`

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const server = await createServer({
    root,
    server: { host: '127.0.0.1', port: 0 },
    plugins: [{
      name: 'stable-upstream-isolated-qa',
      configResolved(config) { config.server.proxy = {} },
      resolveId(id) { return id === '/stable-qa.js' ? '\0stable-qa' : undefined },
      load(id) { return id === '\0stable-qa' ? entry : undefined },
    }],
  })
  await server.listen()
  const base = `http://127.0.0.1:${server.httpServer.address().port}`
  const output = process.env.QA_OUTPUT_DIR
  if (output)
    await mkdir(output, { recursive: true })
  let browser
  try {
    browser = await chromium.launch({
      headless: true,
      ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
    })
    for (const viewport of [{ width: 1440, height: 1000 }, { width: 390, height: 844 }]) {
      const page = await browser.newPage({ viewport })
      await page.emulateMedia({ reducedMotion: 'reduce' })
      const errors = []
      const unexpected = []
      const writes = []
      let completed = false
      page.on('pageerror', error => errors.push(error.message))
      await page.route('**/*', async (route) => {
        const request = route.request()
        const url = new URL(request.url())
        if (url.origin !== base) {
          unexpected.push(url.origin)
          return route.abort()
        }
        if (url.pathname === '/stable-qa') {
          return route.fulfill({
            contentType: 'text/html',
            body: '<!doctype html><html lang="en" data-theme="light"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="app"></div><script type="module" src="/stable-qa.js"></script></html>',
          })
        }
        const path = url.pathname.replace(/^\/dev(?=\/api\/)/, '')
        if (!path.startsWith('/api/'))
          return route.continue()
        if (request.method() !== 'GET')
          writes.push(`${request.method()} ${path}`)
        if (path.endsWith('/events'))
          return route.fulfill({ contentType: 'text/event-stream', body: ': connected\n\n' })
        let body
        if (path.endsWith('/status')) {
          body = {
            currentVersion: completed ? '3.2.0' : '3.1.0',
            needRestart: completed,
            operation: { operationId: 'synthetic-update', kind: 'update', status: completed ? 'succeeded' : 'running' },
          }
        }
        else if (path.endsWith('/detail')) {
          body = {
            currentVersion: '3.1.0',
            latestVersion: '3.2.0',
            hasUpdate: true,
            buildType: 'release',
            updateSupported: true,
            deploymentMode: 'binary',
            notes: '# Synthetic release\n\nStable compatibility fixes.',
          }
        }
        else if (path.endsWith('/version')) {
          body = { version: '3.1.0', latestVersion: '3.2.0', hasUpdate: true, deploymentMode: 'binary' }
        }
        else {
          unexpected.push(path)
          return route.abort()
        }
        return route.fulfill({ contentType: 'application/json', body: JSON.stringify(body) })
      })
      await page.goto(`${base}/stable-qa`)
      const note = page.locator('span[title^="Synthetic current note:"]')
      await note.waitFor()
      assert.equal(await note.evaluate(element => element.scrollWidth > element.clientWidth), true)
      const keyName = page.locator('span[title^="Synthetic Key "]')
      await keyName.waitFor()
      assert.equal(await keyName.evaluate(element => element.scrollWidth > element.clientWidth), true)
      const billing = page.getByRole('button', { name: '查看长上下文计费明细' })
      assert.equal(await billing.count(), 1)
      assert.match(await billing.getAttribute('class'), /warning/)
      await billing.hover()
      const billingTitle = page.getByText('长上下文计费明细', { exact: true })
      await billingTitle.waitFor()
      const billingBounds = await billingTitle.boundingBox()
      assert.ok(billingBounds.x >= 0 && billingBounds.x + billingBounds.width <= viewport.width)
      await page.mouse.move(0, 0)
      assert.equal(await page.getByText('本周期预计总量', { exact: true }).count(), 1)
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
      if (output)
        await page.screenshot({ path: join(output, `usage-forecast-${viewport.width}.png`), fullPage: true, animations: 'disabled' })
      await page.evaluate(() => {
        window.qa.open.value = true
      })
      const dialog = page.getByRole('dialog', { name: '系统更新', exact: true })
      await dialog.waitFor()
      await page.waitForFunction(() => window.qa.store.updating)
      assert.equal(await dialog.getByRole('button', { name: '立即更新', exact: true }).isDisabled(), true)
      await dialog.getByRole('button', { name: '关闭', exact: true }).click()
      await dialog.waitFor({ state: 'detached' })
      completed = true
      await page.evaluate(() => {
        window.qa.open.value = true
      })
      await page.waitForFunction(() => window.qa.store.needRestart)
      await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))
      await page.waitForFunction(() => {
        const panel = document.querySelector('[role="dialog"]')
        return panel && getComputedStyle(panel.parentElement).opacity === '1'
      })
      const bounds = await dialog.boundingBox()
      assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= viewport.width)
      assert.ok(bounds.y >= 0 && bounds.y + bounds.height <= viewport.height)
      assert.equal(await dialog.getByText('Stable compatibility fixes.', { exact: true }).count(), 1)
      if (output)
        await page.screenshot({ path: join(output, `update-${viewport.width}.png`), fullPage: true, animations: 'disabled' })
      assert.deepEqual(writes, [], 'reopening must never submit an update or restart')
      assert.deepEqual(unexpected, [], 'all data must stay in synthetic fixtures')
      assert.deepEqual(errors, [])
      await page.close()
    }
    process.stdout.write('Desktop/mobile notes, cycle forecast and update reopen verification passed.\n')
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
