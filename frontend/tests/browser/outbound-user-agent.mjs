import assert from 'node:assert/strict'
import { mkdir, readFile } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'

const defaultUa = 'Codex Desktop/0.153.4 (Mac OS 15.7.1; arm64) unknown (Codex Desktop; 26.901.51231)'
const endpoint = '/api/admin/settings/openai-user-agent'

function view(selection, defaultUserAgent = defaultUa) {
  const userAgent = selection.userAgent ?? defaultUserAgent
  const [, product, coreVersion, osType, osVersion, arch, terminal, , clientVersion] = userAgent.match(/^(Codex Desktop|codex-tui|codex_exec)\/([^ ]+) \((Mac OS|Ubuntu|Windows) ([^; ]+); ([^ )]+)\) ([^ ]+) \((Codex Desktop|codex-tui|codex_exec); ([^ )]+)\)$/)
  const desktop = product === 'Codex Desktop'
  return {
    ...selection,
    customUserAgent: selection.userAgent ?? null,
    defaultUserAgent,
    effectiveUserAgent: userAgent,
    effectiveDesktopUserAgent: desktop
      ? `Codex Desktop/${clientVersion} (${osType}; ${arch})`
      : 'Codex Desktop/26.901.51231 (Mac OS; arm64)',
    coreVersion,
    desktopVersion: desktop ? clientVersion : '26.901.51231',
    osType,
    osVersion,
    arch,
    terminal,
    verified: selection.mode === 'default',
    defaultVerifiedAt: '2026-09-13T00:00:00Z',
  }
}

async function main() {
  const catalog = JSON.parse(await readFile(new URL('../../src/views/settings/components/outbound-user-agent-samples.json', import.meta.url), 'utf8'))
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    server: { host: '127.0.0.1', port: 0 },
    plugins: [{
      name: 'isolated-outbound-ua-qa',
      configResolved(config) { config.server.proxy = {} },
    }],
  })
  await server.listen()
  const base = `http://127.0.0.1:${server.httpServer.address().port}`
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-outbound-user-agent-qa'
  await mkdir(output, { recursive: true })
  try {
    for (const theme of ['light', 'dark']) {
      for (const width of [1440, 390, 320]) {
        const page = await browser.newPage({ viewport: { width, height: 1000 }, reducedMotion: 'reduce' })
        const errors = []
        const saves = []
        const previews = []
        let saved = { mode: 'default' }
        let latestDefault = defaultUa
        let rejectSave = false
        let releasePreview
        let holdPreview = false
        page.on('pageerror', error => errors.push(error.message))
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
            return route.fulfill({ json: { code: 200, message: 'ok', data: view(saved, latestDefault) } })
          if ((path === endpoint || path === `${endpoint}/preview`) && request.method() === 'POST') {
            const selection = request.postDataJSON()
            if (path.endsWith('/preview')) {
              previews.push(selection)
              if (holdPreview)
                await new Promise(resolve => releasePreview = resolve)
            }
            else {
              saves.push(selection)
              if (rejectSave)
                return route.fulfill({ status: 400, json: { code: 400, message: 'Synthetic save rejection' } })
              saved = selection
            }
            return route.fulfill({ json: { code: 200, message: 'ok', data: view(selection, latestDefault) } })
          }
          errors.push(`unexpected ${request.method()} ${path}`)
          return route.fulfill({ status: 404, json: { message: 'Unexpected fixture route' } })
        })
        await page.goto(`${base}/tests/fixtures/outbound-user-agent.html?theme=${theme}`)
        const input = page.getByRole('textbox', { name: 'OpenAI 出站完整 UA' })
        const automatic = page.getByRole('checkbox', { name: '使用默认 UA，自动更新' })
        const sampleMenu = page.getByRole('combobox', { name: 'UA 样本' })
        const clientMenu = page.getByRole('combobox', { name: '客户端样本' })
        const refresh = page.getByRole('button', { name: '刷新实际配置' })
        const preview = page.getByRole('button', { name: '检查格式并预览' })
        const save = page.getByRole('button', { name: '保存设置' })
        await page.waitForFunction(() => document.querySelector('textarea')?.value.length > 0)
        assert.equal(await automatic.isChecked(), true)
        assert.equal(await input.isDisabled(), true)
        assert.equal(await sampleMenu.count(), 0)
        await automatic.setChecked(false, { force: true })
        await page.getByText('固定版本样本（可选）', { exact: true }).click()
        const samples = width === 1440 && theme === 'light'
          ? catalog.samples
          : ['Desktop', 'CLI', 'Exec'].map(client => catalog.samples.find(sample => sample.client === client))
        for (const sample of samples) {
          await clientMenu.click()
          const clientListbox = page.locator(`#${await clientMenu.getAttribute('aria-controls')}`)
          await clientListbox.getByRole('option', { name: sample.client, exact: true }).click()
          await sampleMenu.click()
          const listbox = page.locator(`#${await sampleMenu.getAttribute('aria-controls')}`)
          await listbox.waitFor()
          const menuBounds = await listbox.boundingBox()
          assert(menuBounds.x >= 0 && menuBounds.x + menuBounds.width <= width, 'menu stays in viewport')
          assert(await listbox.locator('[role="option"] > span:first-child').evaluateAll(labels => labels.every(label => label.scrollWidth <= label.clientWidth)), 'platform labels are not truncated')
          const optionHeights = await listbox.getByRole('option').evaluateAll(options => options.map(option => option.getBoundingClientRect().height))
          assert(optionHeights.every(height => height >= 28), `sample options must not collapse: ${optionHeights}`)
          if (sample === samples[0])
            await page.screenshot({ path: `${output}/menu-${theme}-${width}.png`, fullPage: true })
          await listbox.getByRole('option', { name: `${sample.client} · ${sample.platform}`, exact: true }).click()
          assert.equal(await input.inputValue(), sample.userAgent)
          assert.equal(await automatic.isChecked(), false)
          assert.equal(saves.length, 0)
          assert.equal(previews.length, 0)
          assert.equal(await page.locator('dd').first().textContent().then(text => text.trim()), defaultUa)
          const source = page.getByRole('link')
          assert.equal(await source.getAttribute('href'), catalog.sources[sample.client === 'Desktop' ? 'desktop' : 'cli'].url)
        }
        // Keyboard selection uses the existing accessible select, without publishing.
        await sampleMenu.focus()
        await sampleMenu.press('ArrowDown')
        await sampleMenu.press('ArrowDown')
        await sampleMenu.press('Enter')
        const chosen = await input.inputValue()
        assert(catalog.samples.some(sample => sample.userAgent === chosen))
        assert.equal(saves.length, 0)
        assert.equal(previews.length, 0)

        holdPreview = true
        await Promise.all([
          page.waitForRequest(request => request.url().endsWith(`${endpoint}/preview`)),
          preview.click(),
        ])
        assert.equal(await sampleMenu.isDisabled(), true)
        assert.equal(await clientMenu.isDisabled(), true)
        assert.equal(await automatic.isDisabled(), true)
        assert.equal(await input.isDisabled(), true)
        holdPreview = false
        releasePreview()
        await page.getByText('待保存预览：', { exact: false }).waitFor()
        assert.deepEqual(previews.at(-1), { mode: 'custom', userAgent: chosen })
        assert.equal(saves.length, 0)

        await input.fill(`${chosen} edited`)
        assert.equal(await sampleMenu.textContent().then(text => text.includes('选择样本')), true)
        assert.equal(await page.getByText('待保存预览：', { exact: false }).count(), 0)
        assert.equal(await page.getByRole('link').count(), 0)
        await input.fill(chosen)
        await automatic.setChecked(true, { force: true })
        assert.equal(await input.inputValue(), defaultUa)
        await automatic.setChecked(false, { force: true })
        assert.equal(await input.inputValue(), chosen)
        await page.getByText('固定版本样本（可选）', { exact: true }).click()
        assert.equal(await page.getByRole('link').count(), 1)

        rejectSave = true
        await save.click()
        await page.getByText('Synthetic save rejection', { exact: true }).first().waitFor()
        assert.equal(await input.inputValue(), chosen)
        assert.equal(saved.mode, 'default')
        assert.equal(await input.isDisabled(), false)
        rejectSave = false
        await Promise.all([
          page.waitForResponse(response => response.url().endsWith(endpoint) && response.request().method() === 'POST'),
          save.click(),
        ])
        await page.waitForFunction(ua => document.querySelector('dd')?.textContent.trim() === ua, chosen)
        assert.deepEqual(saved, { mode: 'custom', userAgent: chosen })
        assert.deepEqual(saves, [{ mode: 'custom', userAgent: chosen }, { mode: 'custom', userAgent: chosen }])

        await input.fill('unsaved')
        await Promise.all([
          page.waitForResponse(response => response.url().endsWith(endpoint) && response.request().method() === 'GET'),
          refresh.click(),
        ])
        await page.waitForFunction(ua => document.querySelector('textarea')?.value === ua, chosen)
        assert.equal(await automatic.isChecked(), false)
        assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth))
        for (const control of [input, sampleMenu, clientMenu, save, preview, refresh]) {
          const bounds = await control.boundingBox()
          assert(bounds && bounds.x >= 0 && bounds.x + bounds.width <= width, 'controls fit in viewport')
        }
        await page.screenshot({ path: `${output}/samples-${theme}-${width}.png`, fullPage: true })
        await automatic.setChecked(true, { force: true })
        latestDefault = defaultUa.replace('0.153.4', '0.154.0')
        await Promise.all([
          page.waitForResponse(response => response.url().endsWith(endpoint) && response.request().method() === 'POST'),
          save.click(),
        ])
        await page.waitForFunction(ua => document.querySelector('textarea')?.value === ua, latestDefault)
        assert.deepEqual(saves.at(-1), { mode: 'default' })
        assert.equal(await automatic.isChecked(), true)
        assert.equal(await sampleMenu.count(), 0)
        assert.deepEqual(errors, [])
        await page.close()
      }
    }
    process.stdout.write(`Outbound UA UI passed at 1440/390/320px in both themes; synthetic APIs only. Screenshots: ${output}\n`)
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
