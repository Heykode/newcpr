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
import Timeline from '/src/views/dashboard/components/RequestHealthTimelineCard.vue'
import { useThemeStore } from '/src/stores/modules/theme.ts'
import '@fontsource-variable/inter'
import '@fontsource-variable/jetbrains-mono'
import '/src/styles/index.css'
const pinia = createPinia()
const visible = ref(true)
const timeline = {
  title: 'Request health', description: 'Synthetic timeline',
  reliabilityDisplay: '90%', status: 'stable', successRequests: 90,
  failedRequests: 10, cancelledRequests: 0, callerErrorRequests: 0,
  points: Array.from({ length: 96 }, (_, index) => ({
    time: '12:' + String(index).padStart(2, '0'),
    status: index === 95 ? 'future' : 'stable', reliabilityDisplay: '90%',
    successRequests: index === 95 ? 0 : 9, failedRequests: index === 95 ? 0 : 1,
    cancelledRequests: 0, callerErrorRequests: 0
  }))
}
window.qa = { visible }
const app = createApp({ setup: () => () => h('main', {
  style: 'padding:16px;padding-top:320px;max-width:1100px;margin:auto'
}, visible.value ? [h(Timeline, { timeline })] : []) })
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
      name: 'health-timeline-isolated-qa',
      configResolved(config) { config.server.proxy = {} },
      resolveId(id) { return id === '/timeline-qa.js' ? '\0timeline-qa' : undefined },
      load(id) { return id === '\0timeline-qa' ? entry : undefined },
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
      for (const reducedMotion of ['reduce', 'no-preference']) {
        const page = await browser.newPage({ viewport })
        await page.emulateMedia({ reducedMotion })
        const errors = []
        const unexpected = []
        page.on('pageerror', error => errors.push(error.message))
        await page.route('**/*', async (route) => {
          const url = new URL(route.request().url())
          if (url.origin !== base || url.pathname.startsWith('/api/')) {
            unexpected.push(url.href)
            return route.abort()
          }
          if (url.pathname === '/timeline-qa') {
            return route.fulfill({
              contentType: 'text/html',
              body: '<!doctype html><html lang="en" data-theme="light"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="app"></div><script type="module" src="/timeline-qa.js"></script></html>',
            })
          }
          return route.continue()
        })
        await page.goto(`${base}/timeline-qa`)
        const points = page.locator('[data-health-timeline-point]')
        await points.first().waitFor()
        assert.equal(await points.count(), 96)
        const popover = page.locator('[data-health-timeline-popover]')
        await points.nth(30).hover()
        await popover.waitFor()
        const original = await popover.elementHandle()
        await points.nth(35).hover()
        await page.waitForFunction(() => document.querySelector('[data-health-timeline-popover]')?.textContent.includes('12:35'))
        assert.equal(await original.evaluate(element => element === document.querySelector('[data-health-timeline-popover]')), true, 'cell switches must not remount the panel')
        await popover.hover()
        await page.waitForTimeout(500)
        assert.equal(await popover.isVisible(), true, 'moving into the panel must keep it open')
        for (const index of [0, 94]) {
          await points.nth(index).hover()
          await page.waitForFunction(index => document.querySelector('[data-health-timeline-popover]')?.textContent.includes(`12:${String(index).padStart(2, '0')}`), index)
          await page.waitForTimeout(200)
          if (output)
            await page.screenshot({ path: join(output, `timeline-edge-${viewport.width}-${reducedMotion}-${index}.png`), fullPage: true, animations: 'disabled' })
          assert.equal(await popover.isVisible(), true, `popover closed at ${viewport.width}/${reducedMotion}/${index}`)
          const bounds = await popover.boundingBox()
          assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= viewport.width)
          assert.ok(bounds.y >= 0 && bounds.y + bounds.height <= viewport.height)
        }
        if (output)
          await page.screenshot({ path: join(output, `health-timeline-${viewport.width}-${reducedMotion}.png`), fullPage: true, animations: 'disabled' })
        await points.nth(95).hover()
        await popover.waitFor({ state: 'detached' })
        await points.nth(30).click()
        await popover.waitFor()
        await page.keyboard.press('Escape')
        await popover.waitFor({ state: 'detached' })
        await page.mouse.move(0, 0)
        await points.nth(31).focus()
        await popover.waitFor()
        await page.keyboard.press('Tab')
        await page.waitForFunction(() => document.querySelector('[data-health-timeline-popover]')?.textContent.includes('12:32'))
        await page.setViewportSize({ width: viewport.width - 20, height: viewport.height })
        await page.waitForTimeout(200)
        const resized = await popover.boundingBox()
        if (output)
          await page.screenshot({ path: join(output, `timeline-resized-${viewport.width}-${reducedMotion}.png`), fullPage: true, animations: 'disabled' })
        assert.ok(resized.x >= 0 && resized.x + resized.width <= viewport.width - 20, `resized bounds: ${JSON.stringify({ resized, viewport, reducedMotion, innerWidth: await page.evaluate(() => window.innerWidth) })}`)
        await points.nth(32).evaluate(element => element.blur())
        await popover.waitFor({ state: 'detached' })
        await points.nth(33).hover()
        await popover.waitFor()
        await page.mouse.move(0, 0)
        await popover.waitFor({ state: 'detached' })
        await points.nth(33).hover()
        await popover.waitFor()
        await page.evaluate(() => {
          window.qa.visible.value = false
        })
        await popover.waitFor({ state: 'detached' })
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
        assert.deepEqual(errors, [])
        assert.deepEqual(unexpected, [])
        await page.close()
      }
    }
    process.stdout.write('Desktop/mobile timeline movement, focus, resize and cleanup passed.\n')
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
