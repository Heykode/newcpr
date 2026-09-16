import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-relogin-count-qa'
  await mkdir(output, { recursive: true })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' })
  const errors = []
  const mutations = []
  page.on('pageerror', error => errors.push(error.message))
  page.on('request', (request) => {
    if (request.url().includes('/dev/api/') && request.method() !== 'GET')
      mutations.push(request.url())
  })
  const base = process.env.QA_BASE_URL || 'http://127.0.0.1:5198'
  try {
    await page.goto(`${base}/accounts`)
    await page.locator('td[data-column-key="reloginCount"]').first().waitFor()
    assert.deepEqual(await page.locator('td[data-column-key="reloginCount"]').allTextContents().then(values => values.map(value => value.trim())), ['0', '1', '2'])
    const keys = await page.locator('thead th[data-column-key]').evaluateAll(elements => elements.map(element => element.dataset.columnKey))
    assert.equal(keys[keys.indexOf('lastUsedAt') + 1], 'reloginCount')
    assert.match(await page.locator('td[data-column-key="reloginCount"] span').nth(1).getAttribute('title'), /最近成功重登/)
    for (const direction of ['asc', 'desc']) {
      await Promise.all([
        page.waitForResponse(response => response.url().includes(`sortBy=reloginCount`) && response.url().includes(`sortDirection=${direction}`)),
        page.locator('th[data-column-key="reloginCount"] button').click(),
      ])
    }
    assert.deepEqual(await page.locator('td[data-column-key="reloginCount"]').allTextContents().then(values => values.map(value => value.trim())), ['2', '1', '0'])

    await page.evaluate(() => localStorage.setItem('cpr.accounts.visible-columns', '["identity","lastUsedAt","addedAt"]'))
    await page.reload()
    await page.locator('th[data-column-key="reloginCount"]').waitFor()
    await page.evaluate(() => localStorage.setItem('cpr.accounts.visible-columns', JSON.stringify({ version: 2, keys: ['identity', 'lastUsedAt'] })))
    await page.reload()
    await page.locator('td[data-column-key="identity"]').first().waitFor()
    assert.equal(await page.locator('th[data-column-key="reloginCount"]').count(), 0)
    await page.evaluate(() => localStorage.removeItem('cpr.accounts.visible-columns'))

    for (const route of ['accounts', 'relogin']) {
      await page.goto(`${base}/${route}`)
      await page.locator('td[data-column-key="reloginCount"]').first().waitFor()
      assert.deepEqual(await page.locator('td[data-column-key="reloginCount"]').allTextContents().then(values => values.map(value => value.trim())), ['0', '1', '2'])
      for (const theme of ['light', 'dark']) {
        await page.evaluate(theme => document.documentElement.classList.toggle('dark', theme === 'dark'), theme)
        for (const width of [1920, 1440, 390, 320]) {
          await page.setViewportSize({ width, height: width < 500 ? 844 : 1000 })
          await page.locator('td[data-column-key="reloginCount"]').first().evaluate(element => element.scrollIntoView({ block: 'nearest', inline: 'center' }))
          const headerFits = await page.locator('th[data-column-key="reloginCount"]').evaluate(element =>
            [...element.querySelectorAll('.truncate')].every(label => label.scrollWidth <= label.clientWidth + 1))
          assert.ok(headerFits, `${route} ${width}: count heading must not truncate`)
          await page.screenshot({ path: `${output}/${route}-${theme}-${width}.png`, fullPage: true })
          const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)
          assert.ok(overflow <= 1, `${route} ${width}: document overflow ${overflow}`)
          for (const cell of await page.locator('td[data-column-key="reloginCount"]').all()) {
            const fits = await cell.evaluate(element => element.scrollWidth <= element.clientWidth + 1)
            assert.ok(fits, `${route} ${width}: count must fit`)
          }
        }
      }
    }
    assert.deepEqual(errors, [])
    assert.deepEqual(mutations, [])
    process.stdout.write(`Passed: both pages, counts, timestamps, sorting, preferences, light/dark desktop/mobile. Screenshots: ${output}\n`)
  }
  finally {
    await browser.close()
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack}\n`)
  process.exitCode = 1
})
