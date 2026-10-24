import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'
import { mobileAccounts } from '../fixtures/mobile-account-data.mjs'

const now = new Date().toISOString()
const resetAt = new Date(Date.now() + 3 * 3600_000).toISOString()

function accountRows() {
  return mobileAccounts.slice(0, 3).map((source, index) => {
    const row = structuredClone(source)
    row.id = `acct_overdraft_${index}`
    row.status = 'normal'
    row.enabled = true
    row.inFlight = index === 0 ? 2 : 0
    row.effectiveConcurrencyLimit = 8
    row.quota.windows = row.quota.windows.map((window, windowIndex) => ({
      ...window,
      usedPercent: index === 0 || windowIndex === 0 ? 100 : 76,
      usedPercentDisplay: index === 0 || windowIndex === 0 ? '100%' : '76%',
      limitReached: index === 0 || windowIndex === 0,
      resetAt,
    }))
    row.quota.overdraft = [
      {
        status: 'passed',
        quotaWindow: 'multiple',
        probeModel: 'gpt-6-astra',
        attempts: 1,
        attemptLimit: 5,
        reasonCode: null,
        testedAt: now,
        retryAt: null,
        recoverAt: null,
        windows: row.quota.windows.map((window, windowIndex) => ({
          windowKey: window.key,
          active: true,
          startedAt: now,
          recoverAt: resetAt,
          stats: {
            requests: [28, 53][windowIndex],
            requestsDisplay: `${[28, 53][windowIndex]} 次`,
            tokens: [321000, 604000][windowIndex],
            tokensDisplay: `${[321, 604][windowIndex]}K`,
            costUsd: ['1.46', '2.71'][windowIndex],
            costUsdDisplay: `$${['1.46', '2.71'][windowIndex]}`,
          },
        })),
      },
      {
        status: 'pending',
        quotaWindow: '5h',
        probeModel: 'gpt-6-astra',
        attempts: 2,
        attemptLimit: 5,
        reasonCode: null,
        testedAt: null,
        retryAt: null,
        recoverAt: null,
        windows: [],
      },
      {
        status: 'inconclusive',
        quotaWindow: '5h',
        probeModel: 'gpt-6-astra',
        attempts: 3,
        attemptLimit: 5,
        reasonCode: 'upstream_unavailable',
        testedAt: now,
        retryAt: new Date(Date.now() + 42000).toISOString(),
        recoverAt: null,
        windows: [],
      },
    ][index]
    return row
  })
}

function settings() {
  return {
    modelMappings: {},
    refreshMarginSeconds: 300,
    refreshConcurrency: 4,
    maxConcurrentPerAccount: 8,
    openaiGuardianReservedConcurrency: 0,
    requestIntervalMs: 0,
    responsesMaxDecompressedBodyBytes: 67108864,
    openaiSessionBindingTtlHours: 24,
    openaiAccountAffinity: 'strict',
    rotationStrategy: 'smart',
    disableFast: false,
    accountWarmupEnabled: false,
    accountWarmupScheduleTime: '08:00',
    accountWarmupModel: 'gpt-6-astra',
    codexQuotaOverdraftEnabled: true,
    codexQuotaOverdraftProbeModel: 'gpt-6-astra',
    minCodexDesktopVersion: null,
    minCodexCliVersion: null,
    usageRetentionDays: 31,
    opsEventRetentionDays: 30,
    auditRetentionDays: 90,
    requestTuning: {},
    requestTuningDefaults: {},
    updatedAt: now,
  }
}

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    server: { host: '127.0.0.1', port: 0 },
    plugins: [{ name: 'isolated-overdraft-test', configResolved(config) { config.server.proxy = {} } }],
  })
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-codex-overdraft-qa'
  let browser
  try {
    await server.listen()
    await mkdir(output, { recursive: true })
    const base = `http://127.0.0.1:${server.httpServer.address().port}`
    browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH || undefined })
    for (const [name, width, height] of [['desktop', 1440, 1000], ['mobile', 390, 844]]) {
      const page = await browser.newPage({ locale: 'zh-CN', viewport: { width, height }, reducedMotion: 'reduce' })
      const errors = []
      const unexpected = []
      const rows = accountRows()
      page.on('pageerror', error => errors.push(error.message))
      await page.route('**/*', (route) => {
        const request = route.request()
        const url = new URL(request.url())
        if (url.origin !== base) {
          unexpected.push('external request')
          return route.abort()
        }
        if (!url.pathname.startsWith('/dev/api/'))
          return route.continue()
        const path = url.pathname.slice('/dev'.length)
        const fulfill = data => route.fulfill({ json: { code: 200, message: 'ok', data } })
        if (path === '/api/admin/auth/status' || path === '/api/admin/auth/refresh')
          return fulfill({ authenticated: true, expiresAt: new Date(Date.now() + 3600000).toISOString() })
        if (path === '/api/admin/system/version')
          return fulfill({ version: 'local-test', buildType: 'test' })
        if (path === '/api/admin/settings')
          return fulfill(settings())
        if (path === '/api/admin/settings/admin-api-key')
          return fulfill({ exists: false })
        if (path === '/api/admin/settings/openai-user-agent')
          return fulfill({ mode: 'auto', customUserAgent: null, defaultUserAgent: 'codex_cli_rs/0.159.0', effectiveUserAgent: 'codex_cli_rs/0.159.0', effectiveDesktopUserAgent: 'Codex Desktop/26.1024', coreVersion: '0.159.0', desktopVersion: '26.1024', osType: 'Mac OS', osVersion: '26.4.1', arch: 'arm64', terminal: 'xterm-256color', verified: true, defaultVerifiedAt: now })
        if (path === '/api/admin/notifications/channels')
          return fulfill({ smtp: { enabled: false, host: '', port: 465, security: 'tls', username: null, passwordSet: false, fromName: null, fromEmail: null }, bark: { enabled: false, serverUrl: '', deviceKeySet: false, level: 'active', sound: null, volume: 5, call: false }, lastTest: null, updatedAt: now })
        if (path === '/api/admin/accounts')
          return fulfill({ items: rows, page: { page: 1, pageSize: 20, total: 3, totalPages: 1 }, summary: { total: 3, normal: 3, disabled: 0, error: 0, rateLimited: 0, quotaExhausted: 0 } })
        if (path === '/api/admin/account-groups')
          return fulfill({ items: [], page: { page: 1, pageSize: 200, total: 0, totalPages: 0 } })
        if (path === '/api/admin/account-templates' || path === '/api/admin/accounts/import-tasks')
          return fulfill({ items: [] })
        if (path === '/api/admin/relogin')
          return fulfill({ items: [], settings: { concurrency: 1, paused: false } })
        if (path === '/api/admin/accounts/reset-credits/cache' || path === '/api/admin/accounts/reset-credits/batches')
          return fulfill([])
        if (request.method() === 'POST')
          return fulfill({})
        unexpected.push(`${request.method()} ${path}`)
        return route.abort()
      })

      try {
        await page.goto(`${base}/settings`)
        await page.getByText('Codex 额度透支', { exact: true }).waitFor()
        assert.equal(await page.getByRole('textbox', { name: '透支探测模型', exact: true }).inputValue(), 'gpt-6-astra')
        assert.equal(await page.getByRole('switch', { name: '启用额度透支', exact: true }).isChecked(), true)

        await page.goto(`${base}/accounts`)
        await page.getByText('透支调度中', { exact: true }).waitFor()
        await page.getByText('额度探测中', { exact: true }).waitFor()
        await page.getByText('探测无结论', { exact: true }).waitFor()
        assert.equal(await page.getByText('通用额度', { exact: true }).count(), 0)
        assert.equal(await page.getByText('透支中', { exact: true }).count(), 2)
        assert.equal(await page.getByText('28 次 · 321K · $1.46', { exact: true }).count(), 1)
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1), true)
        assert.deepEqual(errors, [])
        assert.deepEqual(unexpected, [])
        const activeState = rows[0].quota.overdraft
        for (const row of rows)
          row.quota.overdraft = null
        await page.reload()
        await page.locator(width < 768 ? '[data-account-card]' : 'tr[data-row-key]').first().waitFor()
        assert.equal(await page.getByText('透支调度中', { exact: true }).count(), 0)
        assert.equal(await page.getByText('额度探测中', { exact: true }).count(), 0)
        assert.equal(await page.getByText('探测无结论', { exact: true }).count(), 0)

        rows[0].provider = 'xai'
        rows[0].quota.overdraft = activeState
        await page.reload()
        await page.locator(width < 768 ? '[data-account-card]' : 'tr[data-row-key]').first().waitFor()
        assert.equal(await page.getByText('透支调度中', { exact: true }).count(), 0)
        await page.screenshot({ path: `${output}/${name}.png`, fullPage: true, animations: 'disabled' })
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
