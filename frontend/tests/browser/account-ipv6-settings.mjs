import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : 'playwright')
  const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH || undefined })
  const base = process.env.QA_BASE_URL || 'http://127.0.0.1:5216'
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-ipv6-settings-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
  const errors = []
  const batches = []
  const templates = []
  page.on('pageerror', error => errors.push(error.message))
  page.on('request', (request) => {
    if (request.url().endsWith('/accounts/batch-update'))
      batches.push(request.postDataJSON())
    if (request.url().endsWith('/templates/save'))
      templates.push(request.postDataJSON())
  })
  const dialog = page.getByRole('dialog')
  const optIn = () => dialog.getByRole('checkbox', { name: '应用出站隧道更改', exact: true })
  const tunnel = () => dialog.getByRole('combobox', { name: '出站隧道', exact: true })
  const select = () => dialog.getByRole('combobox', { name: 'IPv6 出口策略', exact: true })
  async function chooseTunnel(label) {
    await tunnel().click()
    await page.getByRole('option', { name: label, exact: true }).click()
  }
  async function selectAccounts() {
    for (const id of ['acct_sample_0', 'acct_sample_1'])
      await page.locator(`tr[data-row-key="${id}"]`).getByRole('checkbox', { name: '选择账号', exact: true }).locator('..').click()
  }
  async function layouts(name, control) {
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: width < 500 ? 844 : 1000 })
      await control.scrollIntoViewIfNeeded()
      await page.waitForFunction(() => [...document.querySelectorAll('[role="dialog"]')].every((el) => {
        const bounds = el.getBoundingClientRect()
        return bounds.x >= 0 && bounds.right <= innerWidth + 1
      }))
      assert.ok(await dialog.evaluate(el => el.scrollWidth <= el.clientWidth + 1))
      await control.click()
      await page.getByRole('option', { name: /轮询 IPv6 · 复用连接/ }).waitFor()
      assert.ok(await page.getByRole('option').evaluateAll(options => options.every((option) => {
        const label = option.querySelector('span')
        return label && label.scrollWidth <= label.clientWidth + 1
      })), 'every IPv6 mode label must remain readable')
      await page.screenshot({ path: `${output}/${name}-${width}.png`, fullPage: true })
      await page.getByRole('option', { selected: true }).click()
    }
  }
  try {
    const catalog = await (await page.request.get(`${base}/dev/api/admin/relogin/templates`)).json()
    for (const template of catalog.data.filter(row => row.config.name === 'IPv6 QA template')) {
      await page.request.post(`${base}/dev/api/admin/relogin/templates/delete`, {
        data: { id: template.id, revision: template.revision },
      })
    }
    await page.goto(`${base}/accounts`)
    await page.locator('tr[data-row-key="acct_sample_0"]').waitFor()
    await selectAccounts()
    await page.getByRole('button', { name: '批量编辑账号', exact: true }).click()
    assert.equal(await optIn().isChecked(), false)
    assert.equal(await tunnel().isDisabled(), true)
    await optIn().locator('..').click()
    await chooseTunnel('IPv6 地址池')
    assert.equal(await select().isDisabled(), false)
    await layouts('batch', select())
    await select().click()
    await page.getByRole('option', { name: /轮询 IPv6 · 复用连接/ }).click()
    await dialog.getByRole('button', { name: '保存更改', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.deepEqual(batches.at(-1), { accountIds: ['acct_sample_0', 'acct_sample_1'], requestProxySource: 'account', outboundProxyId: '', egressMode: 'random_ipv6_reuse' })
    await selectAccounts()
    await page.getByRole('button', { name: '批量编辑账号', exact: true }).click()
    assert.equal(await optIn().isChecked(), false)
    assert.equal(await tunnel().isDisabled(), true)
    await optIn().locator('..').click()
    await chooseTunnel('跟随全局策略')
    await dialog.getByRole('button', { name: '保存更改', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.equal(batches.at(-1).egressMode, null)
    await page.getByRole('button', { name: '账号模板', exact: true }).click()
    await page.getByRole('button', { name: '管理模板', exact: true }).click()
    await dialog.getByRole('button', { name: '新建模板', exact: true }).click()
    await dialog.getByRole('textbox', { name: '模板名称', exact: true }).fill('IPv6 QA template')
    const templateSelect = select()
    assert.match(await tunnel().textContent(), /保持原设置/)
    await chooseTunnel('IPv6 地址池')
    await layouts('template', templateSelect)
    await templateSelect.click()
    await page.getByRole('option', { name: /固定 IPv6 · 复用连接/ }).click()
    await dialog.getByRole('button', { name: '保存模板', exact: true }).click()
    await dialog.getByText('IPv6 QA template', { exact: true }).waitFor()
    assert.equal(templates.at(-1).config.egressMode, 'fixed_ipv6_reuse')
    await dialog.getByRole('button', { name: '编辑模板', exact: true }).click()
    assert.match(await templateSelect.textContent(), /固定 IPv6/)
    await chooseTunnel('保持原设置')
    await dialog.getByRole('button', { name: '保存模板', exact: true }).click()
    await dialog.getByText('IPv6 QA template', { exact: true }).waitFor()
    assert.equal('egressMode' in templates.at(-1).config, false)
    assert.deepEqual(errors, [])
    process.stdout.write(`${JSON.stringify({ batches: batches.length, templates: templates.length, screenshots: 6, errors })}\n`)
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
