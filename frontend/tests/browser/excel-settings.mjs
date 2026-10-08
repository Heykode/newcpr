import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'

const root = fileURLToPath(new URL('../..', import.meta.url))
const initial = {
  excelDefaultModels: ['synthetic-model'],
  modelMappings: { 'client-model': 'upstream-model' },
  refreshMarginSeconds: 1800,
  refreshConcurrency: 4,
  maxConcurrentPerAccount: 5,
  requestIntervalMs: 25,
  rotationStrategy: 'smart',
  openaiAccountAffinity: 'strict',
  minCodexDesktopVersion: null,
  minCodexCliVersion: null,
  usageRetentionDays: 31,
  opsEventRetentionDays: 30,
  auditRetentionDays: 90,
  requestTuning: { websocketMaxRetries: 7, maxAccountSwitches: 4, maxRequestAttempts: 9, rateLimitCooldownSeconds: 37 },
  updatedAt: '2026-09-01T00:00:00Z',
}
const mockApi = `
let saved = ${JSON.stringify(initial)};
window.settingsQa = { writes: [] };
export async function getSettings() { return structuredClone(saved); }
export async function updateSettings(payload) {
  const data = structuredClone(payload);
  window.settingsQa.writes.push(data);
  saved = { ...data, updatedAt: ${JSON.stringify(initial.updatedAt)} };
  return structuredClone(saved);
}
export async function getAdminApiKeyStatus() { return { exists: false }; }
export async function deleteAdminApiKey() { throw new Error('unexpected admin mutation'); }
export async function regenerateAdminApiKey() { throw new Error('unexpected admin mutation'); }
`
const entry = `
import { createApp, h } from 'vue';
import { createPinia } from 'pinia';
import { createRouter, createMemoryHistory, RouterView } from 'vue-router';
import SettingsView from '/src/views/settings/index.vue';
import { applyResolvedTheme, DEFAULT_CUSTOM_THEME_COLOR, DEFAULT_THEME_COLOR, resolveTheme } from '/src/theme';
import '@fontsource-variable/inter';
import '@fontsource-variable/jetbrains-mono';
import '/src/styles/index.css';
const theme = new URLSearchParams(location.search).get('theme') === 'dark' ? 'dark' : 'light';
applyResolvedTheme(document.documentElement, resolveTheme(theme, DEFAULT_THEME_COLOR, DEFAULT_CUSTOM_THEME_COLOR, {}));
const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/settings', name: 'settings', component: SettingsView }] });
const app = createApp({ render: () => h('main', { style: 'max-width:1200px;margin:auto;padding:16px;min-width:0' }, [h(RouterView)]) });
app.use(createPinia());
app.use(router);
await router.push('/settings');
await router.isReady();
app.mount('#app');
`

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const server = await createServer({
    root,
    server: { host: '127.0.0.1', port: 0, hmr: false },
    plugins: [{
      name: 'excel-settings-isolated-qa',
      enforce: 'pre',
      configResolved(config) { config.server.proxy = {} },
      resolveId(id, importer) {
        if (id === '/excel-settings-qa.js')
          return '\0excel-settings-qa'
        if (['@/api', join(root, 'src/api'), join(root, 'src/api/index.ts')].includes(id) && /use(?:SettingsForm|AdminApiKey)\.ts$/.test(importer ?? ''))
          return '\0excel-settings-api'
        // These unrelated cards have separate endpoints and are outside this UI test.
        if (importer?.endsWith('/views/settings/index.vue') && /AdminApiKeyCard|AdminPasswordCard|NotificationChannelsCard|OutboundUserAgentCard|SettingsBackupSection/.test(id))
          return '\0excel-settings-unrelated-card'
      },
      load(id) {
        if (id === '\0excel-settings-qa')
          return entry
        if (id === '\0excel-settings-api')
          return mockApi
        if (id === '\0excel-settings-unrelated-card')
          return 'export default { render() { return null } }'
      },
    }],
  })
  await server.listen()
  const base = `http://127.0.0.1:${server.httpServer.address().port}`
  const output = process.env.QA_OUTPUT_DIR
  if (output)
    await mkdir(output, { recursive: true })
  let browser
  try {
    browser = await chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) })
    for (const viewport of [{ width: 1440, height: 1000 }, { width: 390, height: 844 }]) {
      for (const theme of ['light', 'dark']) {
        const page = await browser.newPage({ viewport })
        const errors = []
        const consoleErrors = []
        const unexpected = []
        page.on('pageerror', (error) => {
          errors.push(error.message)
          console.error(error.message)
        })
        page.on('console', (message) => {
          if (message.type() === 'error') {
            consoleErrors.push(message.text())
            console.error(message.text())
          }
        })
        await page.emulateMedia({ reducedMotion: 'reduce' })
        // HMR is test infrastructure only; never connect a socket to a real service.
        await page.routeWebSocket(/.*/, (socket) => {
          const url = new URL(socket.url())
          if (url.hostname === '127.0.0.1' && url.port === new URL(base).port) {
            socket.send(JSON.stringify({ type: 'connected' }))
          }
          else {
            unexpected.push(url.pathname)
            socket.close()
          }
        })
        await page.route('**/*', (route) => {
          const url = new URL(route.request().url())
          if (url.origin !== base || url.pathname.startsWith('/api/') || url.pathname.startsWith('/dev/')) {
            unexpected.push(url.pathname)
            console.error(`Blocked unexpected test request: ${url.pathname}`)
            return route.abort()
          }
          if (url.pathname === '/excel-settings-qa') {
            return route.fulfill({ contentType: 'text/html', body: '<!doctype html><html lang="zh-CN"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="app"></div><script type="module" src="/excel-settings-qa.js"></script></html>' })
          }
          return route.continue()
        })
        await page.goto(`${base}/excel-settings-qa?theme=${theme}`)
        const common = page.getByRole('region', { name: '通用与 Codex 配置', exact: true })
        const excel = page.getByRole('region', { name: 'Excel 配置', exact: true })
        await common.waitFor({ state: 'visible' })
        const affinity = common.getByRole('radiogroup', { name: 'OpenAI 账号亲和模式', exact: true })
        assert.equal(await affinity.getByRole('radio', { name: '严格', exact: true }).isChecked(), true)
        await affinity.getByRole('radio', { name: '优先', exact: true }).click()
        assert.equal(await affinity.getByRole('radio', { name: '优先', exact: true }).isChecked(), true)
        await page.getByRole('button', { name: '共享重试与原生连接高级参数' }).click()
        assert.equal(await page.getByRole('spinbutton', { name: '同账号传输失败重试次数', exact: true }).inputValue(), '7')
        assert.equal(await page.getByRole('spinbutton', { name: '单个请求最多切换账号次数', exact: true }).inputValue(), '4')
        await page.getByRole('spinbutton', { name: '同账号传输失败重试次数', exact: true }).fill('6')
        if (output)
          await page.screenshot({ path: join(output, `common-${viewport.width}-${theme}.png`), fullPage: true })
        await page.getByRole('radio', { name: 'Excel 配置', exact: true }).click()
        await excel.waitFor({ state: 'visible' })
        assert.equal(await common.isVisible(), false)
        const size = page.getByRole('spinbutton', { name: '单张图片上限（MiB）', exact: true })
        assert.equal(await size.inputValue(), '20')
        await size.fill('20.5')
        await page.getByRole('combobox', { name: 'Excel 图片传输方式', exact: true }).click()
        await page.getByRole('option', { name: /^临时 HTTPS 中转/ }).click()
        const url = page.getByRole('textbox', { name: 'Excel 公网 HTTPS 访问地址', exact: true })
        await url.fill('https://synthetic-images.example.com')
        await page.getByRole('combobox', { name: 'Excel 图片传输方式', exact: true }).click()
        await page.getByRole('option', { name: /^BPS 原生附件上传/ }).click()
        assert.equal(await page.getByRole('spinbutton', { name: '进程暂存容量（MiB）', exact: true }).isVisible(), false)
        assert.equal(await size.isVisible(), true)
        await page.getByRole('combobox', { name: 'Excel 图片传输方式', exact: true }).click()
        await page.getByRole('option', { name: /^临时 HTTPS 中转/ }).click()
        assert.equal(await url.inputValue(), 'https://synthetic-images.example.com')
        await page.getByRole('combobox', { name: 'Excel 图片限额处理策略', exact: true }).click()
        await page.getByRole('option', { name: '预警拦截并预留压缩空间', exact: true }).click()
        assert.equal(await page.getByRole('spinbutton', { name: '剩余多少张时预警（张）', exact: true }).inputValue(), '8')
        await page.getByRole('radio', { name: '通用与 Codex 配置', exact: true }).click()
        assert.equal(await page.getByRole('spinbutton', { name: '同账号传输失败重试次数', exact: true }).inputValue(), '6')
        await page.getByRole('radio', { name: 'Excel 配置', exact: true }).click()
        assert.equal(await size.inputValue(), '20.5')
        await size.fill('')
        await page.getByRole('button', { name: '保存全部设置', exact: true }).click()
        assert.equal(await page.evaluate(() => window.settingsQa.writes.length), 0)
        await size.fill('20.5')
        await page.getByRole('button', { name: '保存全部设置', exact: true }).click()
        await page.waitForFunction(() => window.settingsQa.writes.length === 1)
        const saved = await page.evaluate(() => window.settingsQa.writes[0])
        assert.equal(saved.requestTuning.excelImageMaxBytes, 21495808)
        assert.equal(saved.requestTuning.websocketMaxRetries, 6)
        assert.equal(saved.requestTuning.maxAccountSwitches, 4)
        assert.equal(saved.requestTuning.maxRequestAttempts, 9)
        assert.equal(saved.requestTuning.rateLimitCooldownSeconds, 37)
        assert.equal(saved.requestTuning.excelImageTransport.publicUrl, 'https://synthetic-images.example.com')
        assert.equal(saved.rotationStrategy, initial.rotationStrategy)
        assert.equal(saved.openaiAccountAffinity, 'preferred')
        assert.equal(await size.inputValue(), '20.5')
        await page.evaluate(() => document.fonts.ready)
        const overflow = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth)
        assert.equal(overflow, false, `${viewport.width} ${theme} horizontal overflow`)
        const clipped = await excel.locator('input, [role="combobox"]').evaluateAll(elements => elements.filter((element) => {
          const box = element.getBoundingClientRect()
          const container = element.closest('section').getBoundingClientRect()
          return box.width > 0 && (box.left < container.left - 1 || box.right > container.right + 1)
        }).map(element => element.getAttribute('aria-label')))
        assert.deepEqual(clipped, [], 'controls must not be clipped by the card')
        if (output)
          await page.screenshot({ path: join(output, `excel-${viewport.width}-${theme}.png`), fullPage: true })
        assert.deepEqual(errors, [])
        assert.deepEqual(consoleErrors, [])
        assert.deepEqual(unexpected, [])
        console.warn(`PASS ${viewport.width}px ${theme}: real settings page, drafts, MiB save, shared retries, zero external requests`)
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
