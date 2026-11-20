import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'
import { accounts } from '../fixtures/relogin-count-data.mjs'

const plans = [
  ['Free', 'free'],
  ['Go', 'go'],
  ['Plus', 'plus'],
  ['ProLite', 'prolite'],
  ['Pro', 'pro'],
  ['ProMax', 'promax'],
  ['Business', 'team'],
  ['Business Premium', 'self_serve_business_prolite'],
  ['Enterprise', 'enterprise'],
  ['Enterprise (Automation)', 'enterprise_cbp_automation'],
  ['Edu', 'edu'],
  ['Edu Plus', 'edu_plus'],
  ['Edu Pro', 'edu_pro'],
  ['Academic Researchers', 'sci'],
  ['Academic Researchers Plus', 'sci_plus'],
]

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const port = process.env.QA_PORT || '5299'
  const server = spawn(process.execPath, ['tests/relogin-count-preview.mjs'], {
    env: { ...process.env, QA_PORT: port },
    stdio: 'ignore',
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-account-plan-filters'
  await mkdir(output, { recursive: true })
  let browser
  try {
    for (let attempt = 0; attempt < 100; attempt++) {
      try {
        if ((await fetch(`http://127.0.0.1:${port}/accounts`)).ok)
          break
      }
      catch {}
      await new Promise(resolve => setTimeout(resolve, 100))
    }
    browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH || undefined })
    for (const width of [1440, 390, 320]) {
      const page = await browser.newPage({ locale: 'zh-CN', viewport: { width, height: 1000 }, reducedMotion: 'reduce' })
      const errors = []
      page.on('pageerror', error => errors.push(error.message))
      const requests = []
      const rows = plans.map(([label, raw], index) => ({
        ...structuredClone(accounts[0]),
        id: `acct_plan_${index}`,
        email: `plan-${index}@example.invalid`,
        planType: raw,
        planTypeDisplay: label,
      }))
      await page.route('**/dev/api/admin/accounts?*', (route) => {
        const params = new URL(route.request().url()).searchParams
        const filter = params.get('planType')
        requests.push(filter)
        const selected = filter ? rows.filter(row => filter === `openai:${row.planType}`) : []
        return route.fulfill({ json: { code: 200, message: 'ok', data: {
          items: selected,
          page: { page: 1, pageSize: 20, total: selected.length, totalPages: selected.length ? 1 : 0 },
          summary: { total: rows.length, normal: rows.length, error: 0, rateLimited: 0, disabled: 0, quotaExhausted: 0 },
        } } })
      })
      await page.goto(`http://127.0.0.1:${port}/accounts`)
      const trigger = page.getByRole('combobox', { name: '按套餐筛选' })
      await trigger.click()
      const menu = page.locator(`[id="${await trigger.getAttribute('aria-controls')}"]`)
      await menu.getByRole('option', { name: 'Academic Researchers Plus', exact: true }).waitFor()
      assert.deepEqual(await menu.getByRole('option').allTextContents(), ['全部套餐', ...plans.map(([label]) => label)])
      await page.screenshot({ path: `${output}/plan-options-${width}.png` })
      const lastOption = menu.getByRole('option', { name: 'Academic Researchers Plus', exact: true })
      await lastOption.scrollIntoViewIfNeeded()
      const clippedLabels = await menu.getByRole('option').evaluateAll(options => options.flatMap((option) => {
        const label = option.querySelector('span')
        if (!label)
          return [option.textContent]
        return label.scrollWidth > label.clientWidth || label.getBoundingClientRect().height > option.getBoundingClientRect().height
          ? [{ text: label.textContent, width: [label.scrollWidth, label.clientWidth], height: [label.scrollHeight, label.clientHeight] }]
          : []
      }))
      assert.deepEqual(clippedLabels, [], 'all plan names must remain readable without clipping')
      await page.screenshot({ path: `${output}/plan-options-long-${width}.png` })
      await trigger.press('Escape')

      for (const [label, raw] of plans) {
        await trigger.click()
        const response = page.waitForResponse(res => res.url().includes('/dev/api/admin/accounts?')
          && new URL(res.url()).searchParams.get('planType') === `openai:${raw}`)
        await menu.getByRole('option', { name: label, exact: true }).click()
        await response
        const id = rows.find(row => row.planType === raw).id
        await page.locator(width < 768 ? `[data-account-card="${id}"]` : `tr[data-row-key="${id}"]`).waitFor()
        assert.equal(requests.at(-1), `openai:${raw}`)
      }
      const cleared = page.waitForResponse(res => res.url().includes('/dev/api/admin/accounts?')
        && !new URL(res.url()).searchParams.has('planType'))
      await page.getByRole('button', { name: '清除套餐筛选', exact: true }).click()
      await cleared
      await trigger.click()
      assert.equal(await menu.getByRole('option').count(), 16, 'empty results must retain the complete fixed catalog')
      assert.deepEqual(errors, [])
      await page.close()
    }
    process.stdout.write('Codex plan filters: all 15 selections and clearing passed at 1440/390/320px\n')
  }
  finally {
    await browser?.close()
    server.kill('SIGTERM')
    await new Promise(resolve => server.exitCode !== null ? resolve() : server.once('exit', resolve))
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
