import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'

const defaultUa = 'Codex Desktop/0.153.4 (Mac OS 15.7.1; arm64) unknown (Codex Desktop; 26.901.51231)'
const endpoint = '/api/admin/settings/openai-user-agent'
const presets = ['desktop', 'cli'].flatMap(client => ['macos', 'linux', 'windows'].map(platform => ({
  configuration: { client, platform, versionMode: 'latest' },
  automaticAvailable: true,
  reason: null,
  defaults: { originator: client === 'desktop' ? 'Codex Desktop' : 'codex_cli_rs', osType: { macos: 'Mac OS', linux: 'Linux', windows: 'Windows' }[platform], osVersion: { macos: '15.7.1', linux: '6.8', windows: '11.0' }[platform], arch: platform === 'macos' ? 'arm64' : 'x86_64', terminal: 'unknown' },
})))

function view(selection) {
  const config = selection.configuration
  const row = presets.find(row => row.configuration.client === config?.client && row.configuration.platform === config?.platform)
  let userAgent = selection.userAgent ?? defaultUa
  let coreVersion = '0.153.4'
  if (config?.mode === 'custom') {
    userAgent = config.userAgent
    coreVersion = config.codexVersion ?? '0.153.4'
  }
  else if (row) {
    coreVersion = config.versionMode === 'latest' ? '0.180.0' : config.codexVersion
    userAgent = `${config.originator ?? row.defaults.originator}/${coreVersion} (${row.defaults.osType} ${config.osVersion ?? row.defaults.osVersion}; ${config.arch ?? row.defaults.arch}) unknown`
    if (config.client === 'desktop')
      userAgent += ' (Codex Desktop; 26.901.51231)'
  }
  else if (selection.mode === 'custom-auto-version') {
    userAgent = userAgent.replace(/0\.153\.4/g, '0.180.0')
    coreVersion = '0.180.0'
  }
  return {
    ...selection,
    customUserAgent: selection.userAgent ?? null,
    defaultUserAgent: defaultUa,
    effectiveUserAgent: userAgent,
    effectiveDesktopUserAgent: 'Codex Desktop/26.901.51231 (Mac OS; arm64)',
    coreVersion,
    desktopVersion: '26.901.51231',
    osType: row?.defaults.osType ?? 'Mac OS',
    osVersion: config?.osVersion ?? row?.defaults.osVersion ?? '15.7.1',
    arch: config?.arch ?? row?.defaults.arch ?? 'arm64',
    terminal: 'unknown',
    verified: selection.mode === 'default',
    defaultVerifiedAt: '2026-10-31T00:00:00Z',
    profileOptions: { presets, maxVersionLag: 10 },
    profilePreview: config ? { userAgent, codexVersion: coreVersion, desktopVersion: '26.901.51231', desktopBuild: '51231', versionSource: config.versionMode === 'latest' ? 'official' : 'custom', checkedAt: '2026-10-31T00:00:00Z', verifiedAt: null, error: null } : null,
  }
}

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    server: { host: '127.0.0.1', port: 0 },
    plugins: [{ name: 'isolated-ua-qa', configResolved(config) { config.server.proxy = {} } }],
  })
  await server.listen()
  const base = `http://127.0.0.1:${server.httpServer.address().port}`
  const browser = await chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-outbound-user-agent-qa'
  await mkdir(output, { recursive: true })
  try {
    for (const theme of ['light', 'dark']) {
      for (const width of [1440, 390, 320]) {
        const page = await browser.newPage({ viewport: { width, height: 1100 }, reducedMotion: 'reduce' })
        const errors = []
        const saves = []
        let saved = { mode: 'custom', userAgent: defaultUa }
        let rejectSave = false
        page.on('pageerror', (error) => {
          errors.push(error.message)
          console.error(error.message)
        })
        await page.route('**/*', async (route) => {
          const request = route.request()
          const url = new URL(request.url())
          if (url.origin !== base) {
            errors.push(`unexpected external request: ${url.origin}`)
            return route.abort()
          }
          const path = url.pathname.replace(/^\/dev(?=\/api\/)/, '')
          if (!path.startsWith('/api/'))
            return route.continue()
          if (path === endpoint && request.method() === 'GET')
            return route.fulfill({ json: { code: 200, message: 'ok', data: view(saved) } })
          if ([endpoint, `${endpoint}/preview`].includes(path) && request.method() === 'POST') {
            const selection = request.postDataJSON()
            if (path === endpoint) {
              saves.push(selection)
              if (rejectSave)
                return route.fulfill({ status: 400, json: { code: 400, message: 'Synthetic save rejection' } })
              saved = selection
            }
            return route.fulfill({ json: { code: 200, message: 'ok', data: view(selection) } })
          }
          errors.push(`unexpected route ${path}`)
          return route.fulfill({ status: 404 })
        })
        const choose = async (name, option) => {
          const control = page.getByRole('combobox', { name, exact: true })
          await control.click()
          const list = page.locator(`#${await control.getAttribute('aria-controls')}`)
          await list.getByRole('option', { name: option, exact: true }).click()
        }
        const save = page.getByRole('button', { name: '保存设置' })
        const preview = page.getByRole('button', { name: '检查格式并预览' })
        await page.goto(`${base}/tests/fixtures/outbound-user-agent.html?theme=${theme}`)
        await page.waitForFunction(() => document.querySelector('textarea')?.value.length > 0)
        assert.equal(await page.getByRole('textbox', { name: 'OpenAI 出站完整 UA' }).inputValue(), defaultUa)
        assert.equal(saves.length, 0)
        await page.getByRole('checkbox', { name: '自动同步客户端版本', exact: true }).setChecked(true, { force: true })
        await save.click()
        await page.waitForFunction(() => document.querySelector('dd')?.textContent.includes('0.180.0'))
        assert.equal(saved.mode, 'custom-auto-version')
        await choose('UA 配置', '官方客户端预设')
        await choose('客户端', 'Codex CLI')
        await choose('平台', 'Linux')
        assert.equal(await page.getByRole('textbox', { name: '系统版本', exact: true }).inputValue(), '6.8')
        await choose('CLI 入口', 'Exec')
        await preview.click()
        await page.getByText('待保存预览：', { exact: false }).waitFor()
        assert.equal(saves.length, 1)
        await page.getByRole('checkbox', { name: '自动同步客户端版本', exact: true }).setChecked(false, { force: true })
        assert.equal(await page.getByRole('textbox', { name: 'Codex 客户端版本号', exact: true }).inputValue(), '0.180.0')
        await page.getByRole('textbox', { name: 'Codex 客户端版本号', exact: true }).fill('0.179.0')
        rejectSave = true
        await save.click()
        await page.getByText('Synthetic save rejection', { exact: true }).first().waitFor()
        assert.equal(saved.mode, 'custom-auto-version')
        rejectSave = false
        await save.click()
        await page.waitForFunction(() => document.querySelector('dd')?.textContent.includes('0.179.0'))
        assert.equal(saved.mode, 'configuration')
        assert.equal(saved.configuration.versionMode, 'fixed')
        assert.equal(saved.configuration.cliEntry, 'exec')
        assert.equal(saved.configuration.osVersion, '6.8')
        await page.reload()
        await page.getByRole('textbox', { name: '系统版本', exact: true }).waitFor()
        assert.equal(await page.getByRole('textbox', { name: 'Codex 客户端版本号', exact: true }).inputValue(), '0.179.0')
        assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth))
        await page.screenshot({ path: `${output}/preset-${theme}-${width}.png`, fullPage: true })
        await choose('UA 配置', '完整自定义 UA')
        await page.getByRole('textbox', { name: '完整 UA', exact: true }).fill(defaultUa)
        await page.getByRole('textbox', { name: 'Codex 客户端版本号', exact: true }).fill('0.153.4')
        await save.click()
        await page.waitForFunction(expected => document.querySelector('dd')?.textContent.trim() === expected, defaultUa)
        assert.equal(saved.configuration.userAgent, defaultUa)
        assert.equal(saved.configuration.mode, 'custom')
        assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth))
        assert.deepEqual(errors, [])
        await page.screenshot({ path: `${output}/exact-${theme}-${width}.png`, fullPage: true })
        await page.close()
      }
    }
    process.stdout.write('UA browser checks passed: 1440/390/320px, light/dark; synthetic APIs only.\n')
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
