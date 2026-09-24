import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'vite'

const endpoint = '/api/admin/proxies'
const autoLabel = '自动出口地区（地区总开关开启时生效）'
const manualLabel = '手动独立地区（地区总开关开启时生效）'
const manual = { country: 'FR', region: 'Ile-de-France', city: 'Paris', timezone: 'Europe/Paris' }
const manualDraft = { country: 'FR', region: 'Auvergne-Rhone-Alpes', city: 'Lyon', timezone: 'Europe/Paris' }
const detected = { country: 'JP', region: 'Tokyo', city: 'Tokyo', timezone: 'Asia/Tokyo' }
const refreshed = { country: 'AU', region: 'New South Wales', city: 'Sydney', timezone: 'Australia/Sydney' }
const failedMessage = '模拟地区查询失败'
const conflictMessage = 'IPv4 与 IPv6 出口时区不同，自动地区不可用'
const revisionMessage = '代理配置已变更，请刷新后重试'
const unsavedModeMessage = '请先保存自动出口地区设置，再测试连接'
const timestamp = '2026-09-24T00:00:00Z'
const fields = { country: '国家代码', region: '地区', city: '城市', timezone: '时区' }

function probeResult(location = { status: 'notRequested' }) {
  return {
    success: true,
    latencyMs: 12,
    exitIp: '192.0.2.40',
    exitIpv4: '192.0.2.40',
    exitIpv6: '2001:db8::40',
    message: 'Synthetic connectivity success',
    location,
  }
}

function detection(location) {
  return { location: structuredClone(location), exitIpv4: '192.0.2.40', exitIpv6: '2001:db8::40', detectedAt: timestamp }
}

function record(id, name, overrides = {}) {
  return {
    id,
    name,
    endpoint: 'http://127.0.0.1:8080',
    hasAuthentication: false,
    revision: 1,
    accountCount: 0,
    lastTestAt: null,
    lastTest: null,
    createdAt: timestamp,
    updatedAt: timestamp,
    ...overrides,
  }
}

function fixture() {
  const records = [
    // Missing optional fields reproduce a server predating automatic location.
    record('legacy-manual', 'Legacy manual fixture', { requestLocation: structuredClone(manual), revision: 3 }),
    record('existing-auto', 'Existing auto fixture', {
      autoLocation: true,
      requestLocation: structuredClone(manual),
      detectedLocation: detection(detected),
      effectiveLocation: structuredClone(detected),
      lastTest: probeResult({ status: 'detected', location: detected }),
      lastTestAt: timestamp,
      revision: 7,
      hasAuthentication: true,
    }),
  ]
  const calls = []
  const errors = []
  let nextId = 0
  let configRevision = 1
  let heldTest = null
  let releaseTest = () => {}
  const state = {
    records,
    calls,
    errors,
    layoutErrors: [],
    nextLocation: { status: 'detected', location: refreshed },
    rejectUpdate: false,
    pauseTest() {
      heldTest = new Promise(resolve => releaseTest = resolve)
      return () => {
        releaseTest()
        heldTest = null
      }
    },
    release: () => releaseTest(),
    mutations: suffix => calls.filter(call => call.method === 'POST' && call.path === `${endpoint}/${suffix}`),
    async route(route, base) {
      const request = route.request()
      const url = new URL(request.url())
      if (url.origin !== base) {
        errors.push(`blocked external ${request.method()} ${url.origin}`)
        return route.abort()
      }
      const path = url.pathname.replace(/^\/dev(?=\/api\/)/, '')
      if (!path.startsWith('/api/'))
        return route.continue()
      const method = request.method()
      const body = method === 'POST' ? request.postDataJSON() : null
      calls.push({ path, method, body: structuredClone(body) })
      const ok = data => route.fulfill({ json: { code: 200, message: 'ok', data } })
      const conflict = () => route.fulfill({ status: 409, json: { code: 40901, message: revisionMessage, data: null } })
      try {
        if (path === '/api/admin/auth/status' && method === 'GET')
          return ok({ authenticated: true })
        if (path === '/api/admin/system/version' && method === 'GET')
          return ok({ version: 'test', hasUpdate: false, deploymentMode: 'test' })
        if (path === endpoint && method === 'GET')
          return ok({ items: records, page: { page: 1, pageSize: 20, total: records.length, totalPages: 1 } })
        if (path === `${endpoint}/probe` && method === 'POST') {
          assert.deepEqual(Object.keys(body).sort(), ['detectLocation', 'proxyUrl'])
          assert.equal(typeof body.detectLocation, 'boolean')
          return ok(probeResult(body.detectLocation ? { status: 'detected', location: refreshed } : { status: 'notRequested' }))
        }
        if (path === `${endpoint}/create` && method === 'POST') {
          assert.deepEqual(Object.keys(body).sort(), ['autoLocation', 'name', 'proxyUrl', 'requestLocation'])
          const item = record(`created-${++nextId}`, body.name, {
            endpoint: body.proxyUrl,
            autoLocation: body.autoLocation,
            requestLocation: structuredClone(body.requestLocation),
            detectedLocation: body.autoLocation ? detection(detected) : null,
            effectiveLocation: structuredClone(body.autoLocation ? detected : body.requestLocation),
            lastTest: body.autoLocation ? probeResult({ status: 'detected', location: detected }) : null,
            lastTestAt: body.autoLocation ? timestamp : null,
          })
          records.push(item)
          return ok({ record: item, configRevision: ++configRevision })
        }
        if ((path === `${endpoint}/test` || path === `${endpoint}/update`) && method === 'POST') {
          const index = records.findIndex(item => item.id === body.id)
          assert(index >= 0, 'mutation must name an existing fixture')
          const previous = records[index]
          if (body.revision !== previous.revision)
            return conflict()
          if (path.endsWith('/test')) {
            assert.deepEqual(Object.keys(body).sort(), ['id', 'revision'])
            if (heldTest)
              await heldTest
            const result = probeResult(previous.autoLocation
              ? structuredClone(state.nextLocation)
              : { status: 'notRequested' })
            // Match the backend's same-egress failure retention and conflict invalidation.
            const nextDetected = result.location.status === 'detected'
              ? detection(result.location.location)
              : result.location.status === 'conflict' ? null : previous.detectedLocation
            records[index] = {
              ...previous,
              revision: previous.revision + 1,
              lastTest: result,
              lastTestAt: timestamp,
              detectedLocation: nextDetected,
              effectiveLocation: previous.autoLocation ? nextDetected?.location ?? null : previous.requestLocation,
            }
            return ok(records[index])
          }
          assert.deepEqual(Object.keys(body).sort(), ['autoLocation', 'id', 'name', 'requestLocation', 'revision'])
          if (state.rejectUpdate) {
            previous.revision++
            return conflict()
          }
          const nextDetected = previous.autoLocation === body.autoLocation ? previous.detectedLocation : null
          records[index] = {
            ...previous,
            name: body.name,
            revision: previous.revision + 1,
            autoLocation: body.autoLocation,
            requestLocation: structuredClone(body.requestLocation),
            detectedLocation: nextDetected,
            effectiveLocation: body.autoLocation ? nextDetected?.location ?? null : structuredClone(body.requestLocation),
            lastTest: previous.lastTest && {
              ...previous.lastTest,
              location: previous.autoLocation === body.autoLocation ? previous.lastTest.location : { status: 'notRequested' },
            },
          }
          return ok({ record: records[index], configRevision: ++configRevision })
        }
        errors.push(`unexpected ${method} ${path}`)
        return route.fulfill({ status: 404, json: { code: 404, message: 'Unexpected fixture route', data: null } })
      }
      catch (error) {
        errors.push(`${method} ${path}: ${error.message}`)
        return route.fulfill({ status: 500, json: { code: 500, message: 'Fixture contract violation', data: null } })
      }
    },
  }
  return state
}

async function post(page, suffix, button) {
  const [response] = await Promise.all([
    page.waitForResponse(response => new URL(response.url()).pathname.endsWith(`${endpoint}/${suffix}`) && response.request().method() === 'POST'),
    button.click(),
  ])
  return response
}

async function fillLocation(dialog, value) {
  for (const [key, label] of Object.entries(fields))
    await dialog.getByRole('textbox', { name: label, exact: true }).fill(value[key])
}

async function assertLocation(dialog, value, disabled) {
  for (const [key, label] of Object.entries(fields)) {
    const input = dialog.getByRole('textbox', { name: label, exact: true })
    assert.equal(await input.inputValue(), value[key], label)
    assert.equal(await input.isDisabled(), disabled, `${label} disabled`)
  }
}

async function setSwitch(input, enabled) {
  if (await input.isChecked() !== enabled) {
    assert.equal(await input.isDisabled(), false)
    // BaseSwitch's native checkbox is sr-only; click its visible wrapping label.
    await input.locator('..').click()
  }
  assert.equal(await input.isChecked(), enabled)
}

async function clearNotices(page) {
  const buttons = page.getByRole('button', { name: /^关闭(成功|警告|失败|信息)通知$/ })
  while (await buttons.count()) {
    try {
      await buttons.first().click({ timeout: 1000 })
    }
    catch (error) {
      // A transient notice may expire while Playwright checks actionability.
      if (await buttons.count())
        throw error
    }
  }
}

async function capture(page, output, width, phase, layoutErrors) {
  await page.screenshot({ path: `${output}/${phase}-${width}.png`, fullPage: true })
  const violations = await page.evaluate(() => {
    const failures = []
    if (document.documentElement.scrollWidth > innerWidth)
      failures.push('document overflows viewport')
    for (const dialog of document.querySelectorAll('[role="dialog"]')) {
      const panel = dialog.getBoundingClientRect()
      if (panel.left < 0 || panel.right > innerWidth || dialog.scrollWidth > dialog.clientWidth + 1)
        failures.push('dialog overflows viewport')
      for (const control of dialog.querySelectorAll('input, button, label, p, footer')) {
        const rect = control.getBoundingClientRect()
        if (rect.width && (rect.left < panel.left - 1 || rect.right > panel.right + 1))
          failures.push(`${control.tagName}: ${control.getAttribute('aria-label') || control.textContent.trim().slice(0, 50)}`)
      }
      const footer = dialog.querySelector('footer')
      if (footer?.getBoundingClientRect().bottom > innerHeight)
        failures.push('footer is outside viewport')
    }
    for (const notice of document.querySelectorAll('article[role="status"], article[role="alert"]')) {
      const rect = notice.getBoundingClientRect()
      if (rect.left < 0 || rect.right > innerWidth)
        failures.push('notification overflows viewport')
      for (const text of notice.querySelectorAll('p')) {
        if (text.scrollWidth > text.clientWidth + 1 || text.scrollHeight > text.clientHeight + 1)
          failures.push(`clipped notification: ${text.textContent.trim()}`)
      }
    }
    return failures
  })
  // Finish the lifecycle even if a visual regression exists in an earlier state.
  layoutErrors.push(...violations.map(violation => `${phase}: ${violation}`))
}

async function runViewport(page, state, base, output, width) {
  await page.goto(`${base}/proxies`)
  const row = id => page.locator(`tr[data-row-key="${id}"]`)
  const dialog = page.getByRole('dialog')
  const auto = dialog.getByRole('switch', { name: autoLabel, exact: true })
  const manualSwitch = dialog.getByRole('switch', { name: manualLabel, exact: true })
  const name = dialog.getByRole('textbox', { name: '代理名称', exact: true })
  const url = dialog.getByLabel('代理 URL', { exact: true })
  const test = dialog.getByRole('button', { name: '测试连接', exact: true })
  const save = dialog.getByRole('button', { name: '保存代理', exact: true })
  const cancel = dialog.getByRole('button', { name: '取消', exact: true })
  const locationCell = id => row(id).locator('[data-column-key="location"]')
  const snapshot = phase => capture(page, output, width, phase, state.layoutErrors)
  const openNew = async () => {
    await clearNotices(page)
    await page.getByRole('button', { name: '新增代理', exact: true }).click()
    await dialog.getByRole('heading', { name: '新增代理', exact: true }).waitFor()
  }
  const edit = async (id) => {
    await clearNotices(page)
    await row(id).getByRole('button', { name: '编辑代理', exact: true }).click()
    await dialog.getByRole('heading', { name: '编辑代理', exact: true }).waitFor()
  }
  const saveAndClose = async (suffix) => {
    const response = await post(page, suffix, save)
    assert.equal(response.status(), 200)
    await dialog.waitFor({ state: 'hidden' })
  }
  const rejectUnsavedModeTest = async (phase) => {
    const recordsBefore = structuredClone(state.records)
    const mutationsBefore = state.calls.filter(call => call.method === 'POST')
    await test.click()
    const notice = page.getByRole('status').filter({ hasText: unsavedModeMessage })
    await notice.getByText('警告', { exact: true }).waitFor()
    await notice.getByText(unsavedModeMessage, { exact: true }).waitFor()
    assert.equal(await url.inputValue(), '')
    assert.equal(await test.isDisabled(), false)
    assert.equal(await save.isDisabled(), false)
    await snapshot(phase)
    assert.deepEqual(state.calls.filter(call => call.method === 'POST'), mutationsBefore, 'unsaved mode plus empty URL cannot test, probe or publish')
    assert.deepEqual(state.records, recordsBefore, 'guard cannot mutate the saved proxy')
    await clearNotices(page)
  }

  await row('legacy-manual').waitFor()
  assert.equal(state.calls.filter(call => call.method === 'POST').length, 0)
  await edit('legacy-manual')
  assert.equal(await auto.isChecked(), false, 'legacy omitted autoLocation stays off')
  assert.equal(await manualSwitch.isChecked(), true)
  await assertLocation(dialog, manual, false)
  await setSwitch(auto, true)
  await rejectUnsavedModeTest('unsaved-enable-blocked')
  await assertLocation(dialog, manual, true)
  const legacyBeforeProbe = structuredClone(state.records)
  await url.fill('http://127.0.0.1:8098')
  assert.equal((await post(page, 'probe', test)).status(), 200)
  assert.deepEqual(state.mutations('probe').at(-1).body, { proxyUrl: 'http://127.0.0.1:8098', detectLocation: true })
  assert.equal(state.mutations('test').length, 0, 'new URL uses the enabled draft instead of the saved manual mode')
  assert.deepEqual(state.records, legacyBeforeProbe)
  await url.fill('')
  await setSwitch(auto, false)
  await clearNotices(page)
  assert.equal((await post(page, 'test', test)).status(), 200)
  await page.getByRole('status').filter({ hasText: 'Legacy manual fixture：连接成功' }).waitFor()
  assert.deepEqual(state.mutations('test').at(-1).body, { id: 'legacy-manual', revision: 3 })
  await assertLocation(dialog, manual, false)
  await snapshot('restored-manual-test')
  await cancel.click()
  await dialog.waitFor({ state: 'hidden' })

  const probesBeforeDefaultSave = state.mutations('probe').length
  await openNew()
  assert.equal(await auto.isChecked(), false, 'new proxy defaults off')
  assert.equal(await manualSwitch.isChecked(), false)
  await name.fill('Default fixture')
  await url.fill('http://127.0.0.1:8081')
  await snapshot('default-off')
  await saveAndClose('create')
  assert.deepEqual(state.mutations('create').at(-1).body, {
    name: 'Default fixture',
    proxyUrl: 'http://127.0.0.1:8081',
    autoLocation: false,
    requestLocation: null,
  })
  assert.equal(state.mutations('probe').length, probesBeforeDefaultSave, 'saving default must not add a frontend probe')
  await locationCell('created-1').getByText('继承全局', { exact: true }).waitFor()

  await openNew()
  assert.equal(await auto.isChecked(), false)
  await name.fill('New auto fixture')
  await url.fill('http://127.0.0.1:8082')
  await setSwitch(manualSwitch, true)
  await fillLocation(dialog, manual)
  for (const enabled of [false, true, false]) {
    await setSwitch(auto, enabled)
    await assertLocation(dialog, manual, enabled)
    assert.equal(await manualSwitch.isDisabled(), enabled)
    const previousRecords = structuredClone(state.records)
    assert.equal((await post(page, 'probe', test)).status(), 200)
    assert.deepEqual(state.mutations('probe').at(-1).body, { proxyUrl: 'http://127.0.0.1:8082', detectLocation: enabled })
    assert.deepEqual(state.records, previousRecords, 'URL probe cannot publish or replace saved records')
    await assertLocation(dialog, manual, enabled)
    await clearNotices(page)
  }
  await setSwitch(auto, true)
  await snapshot('create-auto')
  await saveAndClose('create')
  assert.deepEqual(state.mutations('create').at(-1).body, {
    name: 'New auto fixture',
    proxyUrl: 'http://127.0.0.1:8082',
    autoLocation: true,
    requestLocation: manual,
  })
  await locationCell('created-2').getByText('Tokyo · Asia/Tokyo', { exact: true }).waitFor()
  assert.equal(state.mutations('probe').length, 4)
  assert.equal(state.mutations('test').length, 1)

  await edit('existing-auto')
  assert.equal(await auto.isChecked(), true)
  assert.equal(await url.inputValue(), '', 'editing must not expose or resend saved authentication')
  await assertLocation(dialog, manual, true)
  await setSwitch(auto, false)
  await rejectUnsavedModeTest('unsaved-disable-blocked')
  const automaticBeforeProbe = structuredClone(state.records)
  await url.fill('http://127.0.0.1:8097')
  assert.equal((await post(page, 'probe', test)).status(), 200)
  assert.deepEqual(state.mutations('probe').at(-1).body, { proxyUrl: 'http://127.0.0.1:8097', detectLocation: false })
  assert.equal(state.mutations('test').length, 1, 'new URL uses the disabled draft instead of the saved automatic mode')
  assert.deepEqual(state.records, automaticBeforeProbe)
  await url.fill('')
  await clearNotices(page)
  await fillLocation(dialog, manualDraft)
  await name.fill('Edited auto fixture')
  await setSwitch(auto, true)
  await assertLocation(dialog, manualDraft, true)
  await url.fill('http://127.0.0.1:8099')
  await post(page, 'probe', test)
  assert.deepEqual(state.mutations('probe').at(-1).body, { proxyUrl: 'http://127.0.0.1:8099', detectLocation: true })
  assert.equal(state.mutations('test').length, 1, 'entered URL must not test the old saved endpoint')
  await url.fill('')
  const release = state.pauseTest()
  try {
    await Promise.all([
      page.waitForRequest(request => new URL(request.url()).pathname.endsWith(`${endpoint}/test`)),
      test.click(),
    ])
    assert.equal(await auto.isDisabled(), true)
    assert.equal(await name.isDisabled(), true)
    assert.equal(await url.isDisabled(), true)
    assert.equal(await save.isDisabled(), true)
    assert.equal(await cancel.isDisabled(), true)
    assert.equal(state.records.find(item => item.id === 'existing-auto').revision, 7)
  }
  finally {
    release()
  }
  await dialog.getByText('Sydney · Australia/Sydney', { exact: true }).waitFor()
  await locationCell('existing-auto').getByText('Sydney · Australia/Sydney', { exact: true }).waitFor()
  assert.deepEqual(state.mutations('test').at(-1).body, { id: 'existing-auto', revision: 7 })
  assert.equal(await name.inputValue(), 'Edited auto fixture')
  await assertLocation(dialog, manualDraft, true)
  await snapshot('tested-revision')
  await saveAndClose('update')
  assert.deepEqual(state.mutations('update').at(-1).body, {
    id: 'existing-auto',
    revision: 8,
    name: 'Edited auto fixture',
    autoLocation: true,
    requestLocation: manualDraft,
  })
  await row('existing-auto').getByText('Edited auto fixture', { exact: true }).waitFor()

  await edit('existing-auto')
  await setSwitch(auto, false)
  await assertLocation(dialog, manualDraft, false)
  assert.equal(await manualSwitch.isChecked(), true)
  await setSwitch(auto, true)
  await assertLocation(dialog, manualDraft, true)
  await setSwitch(auto, false)
  await assertLocation(dialog, manualDraft, false)
  await snapshot('manual-restored')
  await saveAndClose('update')
  assert.deepEqual(state.mutations('update').at(-1).body, {
    id: 'existing-auto',
    revision: 9,
    name: 'Edited auto fixture',
    autoLocation: false,
    requestLocation: manualDraft,
  })
  await locationCell('existing-auto').getByText('手动', { exact: true }).waitFor()
  await locationCell('existing-auto').getByText('Lyon · Europe/Paris', { exact: true }).waitFor()

  await edit('created-2')
  const oldDetection = structuredClone(state.records.find(item => item.id === 'created-2').detectedLocation)
  state.nextLocation = { status: 'failed', message: failedMessage }
  await post(page, 'test', test)
  const retainedMessage = `${failedMessage}；保留 Asia/Tokyo`
  await dialog.getByText(retainedMessage, { exact: true }).waitFor()
  await locationCell('created-2').getByText(retainedMessage, { exact: true }).waitFor()
  const failedNotice = page.getByRole('status').filter({ hasText: `连接成功；${failedMessage}` })
  await failedNotice.getByText('警告', { exact: true }).waitFor()
  await failedNotice.getByText(`连接成功；${failedMessage}`, { exact: true }).waitFor()
  assert.deepEqual(state.records.find(item => item.id === 'created-2').detectedLocation, oldDetection)
  await assertLocation(dialog, manual, true)
  await snapshot('failed-retained')
  await clearNotices(page)

  state.nextLocation = { status: 'conflict' }
  await post(page, 'test', test)
  await dialog.getByText(conflictMessage, { exact: true }).waitFor()
  await locationCell('created-2').getByText(conflictMessage, { exact: true }).waitFor()
  const conflictNotice = page.getByRole('status').filter({ hasText: `连接成功；${conflictMessage}` })
  await conflictNotice.getByText('警告', { exact: true }).waitFor()
  await conflictNotice.getByText(`连接成功；${conflictMessage}`, { exact: true }).waitFor()
  assert.equal(state.records.find(item => item.id === 'created-2').detectedLocation, null)
  assert.equal(state.records.find(item => item.id === 'created-2').effectiveLocation, null)
  assert.equal(await dialog.getByText(retainedMessage, { exact: true }).count(), 0)
  await assertLocation(dialog, manual, true)
  await snapshot('location-conflict')
  await clearNotices(page)

  state.rejectUpdate = true
  await name.fill('Unsaved conflict draft')
  const updatesBefore = state.mutations('update').length
  assert.equal((await post(page, 'update', save)).status(), 409)
  await page.getByRole('alert').filter({ hasText: revisionMessage }).waitFor()
  assert.equal(await dialog.isVisible(), true)
  assert.equal(await name.inputValue(), 'Unsaved conflict draft')
  assert.equal(await auto.isChecked(), true)
  await assertLocation(dialog, manual, true)
  assert.equal(state.records.find(item => item.id === 'created-2').name, 'New auto fixture')
  assert.equal(state.mutations('update').at(-1).body.revision, 3, 'two completed tests must advance the edit revision')
  assert.equal(state.mutations('update').length, updatesBefore + 1, 'uncertain writes must not be retried')
  await snapshot('revision-conflict')
  await cancel.click()
  await dialog.waitFor({ state: 'hidden' })
  await clearNotices(page)
  await snapshot('proxy-list')
  assert.deepEqual(state.errors, [])
  assert.equal(state.mutations('create').length, 2)
  assert.equal(state.mutations('probe').length, 6)
  assert.equal(state.mutations('test').length, 4)
  assert.equal(state.mutations('update').length, 3)
  assert.deepEqual(state.layoutErrors, [], `${width}px layout violations after completed lifecycle`)
}

async function main() {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : 'playwright')
  const output = process.env.QA_OUTPUT_DIR || '/tmp/cpr-proxy-auto-location-qa'
  await mkdir(output, { recursive: true })
  const server = await createServer({
    root: fileURLToPath(new URL('../..', import.meta.url)),
    cacheDir: `${output}/vite-cache`,
    server: { host: '127.0.0.1', port: 0 },
    plugins: [{
      name: 'isolated-proxy-auto-location-qa',
      configResolved(config) { config.server.proxy = {} },
    }],
  })
  let browser
  try {
    await server.listen()
    const base = `http://127.0.0.1:${server.httpServer.address().port}`
    browser = await chromium.launch({
      headless: true,
      ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
    })
    const failures = []
    for (const width of [1440, 390, 320]) {
      const context = await browser.newContext({
        viewport: { width, height: 1000 },
        colorScheme: 'light',
        reducedMotion: 'reduce',
        serviceWorkers: 'block',
      })
      const state = fixture()
      const page = await context.newPage()
      page.setDefaultTimeout(15_000)
      page.on('pageerror', error => state.errors.push(error.message))
      await context.route('**/*', route => state.route(route, base))
      try {
        await runViewport(page, state, base, output, width)
        process.stdout.write(`PASS ${width}px: default/create/probe/draft-mode-guard/revision/manual/failed/conflict; synthetic APIs only.\n`)
      }
      catch (error) {
        await page.screenshot({ path: `${output}/failure-${width}.png`, fullPage: true })
        failures.push(new Error(`${width}px: ${error.message}; fixture errors: ${JSON.stringify(state.errors)}`, { cause: error }))
      }
      finally {
        state.release()
        await context.close()
      }
    }
    if (failures.length)
      throw new AggregateError(failures, 'Proxy auto-location browser regression failed')
    process.stdout.write(`Screenshots: ${output}\n`)
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
