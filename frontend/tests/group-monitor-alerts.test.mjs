/* eslint-disable test/no-import-node-test -- uses the project's Node test runner. */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { test } from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from 'typescript'
import * as vue from 'vue'
import { compileScript, parse } from 'vue/compiler-sfc'

const require = createRequire(import.meta.url)

function source(path) {
  return readFileSync(new URL(path, import.meta.url), 'utf8')
}

function loadPresentation() {
  const { outputText } = ts.transpileModule(source('../src/utils/notification-presentation.ts'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  const exports = {}
  runInNewContext(outputText, { exports })
  return exports
}

function loadApi() {
  const calls = []
  const { outputText } = ts.transpileModule(source('../src/api/modules/notifications.ts'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  const exports = {}
  runInNewContext(outputText, {
    exports,
    require: name => name === '../request' ? async request => calls.push(request) : require(name),
  })
  return { api: exports, calls }
}

function channelHarness(smtp = {}) {
  const calls = { reads: 0, saves: [], tests: [] }
  const errors = []
  const failures = { save: false, send: false, reload: false }
  let saved = {
    smtp: { enabled: true, host: 'smtp.example.com', port: 587, security: 'starttls', username: null, passwordSet: false, fromName: null, fromEmail: 'alerts@example.com', ...smtp },
    bark: { enabled: true, serverUrl: 'https://push.example.com', deviceKeySet: true, level: 'active', sound: null, volume: 5, call: false },
    lastTest: null,
    updatedAt: '2026-09-23T00:00:00Z',
  }
  const snapshot = value => structuredClone(vue.toRaw(value))
  const { descriptor } = parse(source('../src/views/settings/components/NotificationChannelsCard.vue'))
  const compiled = compileScript(descriptor, { id: 'notification-channels-test' })
  const { outputText } = ts.transpileModule(compiled.content, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2024 },
  })
  const dependencies = {
    'vue': { ...vue, onMounted: () => {} },
    '@vueuse/core': { useLocalStorage: (_key, fallback) => vue.ref(fallback) },
    '@lucide/vue': {},
    '@/api': {
      getNotificationChannels: async () => {
        calls.reads += 1
        if (failures.reload)
          throw new Error('Synthetic reload failure')
        return snapshot(saved)
      },
      updateNotificationChannels: async (form) => {
        const body = snapshot(form)
        calls.saves.push(body)
        if (failures.save)
          throw new Error('Synthetic save failure')
        saved = snapshot(body)
        saved.smtp.passwordSet ||= Boolean(saved.smtp.password)
        saved.bark.deviceKeySet ||= Boolean(saved.bark.deviceKey)
        delete saved.smtp.password
        delete saved.bark.deviceKey
        return snapshot(saved)
      },
      testNotification: async (body) => {
        calls.tests.push(snapshot(body))
        saved.lastTest = {
          id: 'synthetic-test',
          channel: body.channel,
          target: body.target,
          status: failures.send ? 'failed' : 'sent',
          test: true,
          attempts: 1,
          createdAt: saved.updatedAt,
          finishedAt: saved.updatedAt,
        }
        if (failures.send)
          throw new Error('Synthetic delivery failure')
      },
    },
    '@/components/base/BaseToast': { toast: { error: message => errors.push(message), success: () => {} } },
    '@/utils/async': { errorMessage: error => error.message },
    '@/utils/notification-presentation': loadPresentation(),
  }
  const exports = {}
  runInNewContext(outputText, {
    exports,
    require: name => name.endsWith('.vue') ? {} : dependencies[name] ?? require(name),
  })
  const scope = vue.effectScope()
  const state = scope.run(() => exports.default.setup({}, { expose: () => {} }))
  return { state, calls, errors, failures, snapshot, stop: () => scope.stop() }
}

test('unconfigured SMTP starts with paired TLS and 465 defaults and saves that pair', async (t) => {
  const h = channelHarness({ enabled: false, host: '', fromEmail: null })
  t.after(h.stop)
  assert.equal(h.state.form.smtp.security, 'tls')
  assert.equal(h.state.form.smtp.port, 465)
  await h.state.load()
  assert.equal(h.state.form.smtp.security, 'tls')
  assert.equal(h.state.form.smtp.port, 465)
  assert.equal(h.calls.saves.length, 0)
  await h.state.save()
  assert.equal(h.calls.saves[0].smtp.security, 'tls')
  assert.equal(h.calls.saves[0].smtp.port, 465)
  await h.state.load()
  assert.equal(h.state.form.smtp.security, 'tls')
  assert.equal(h.state.form.smtp.port, 465)
  h.state.form.smtp.security = 'starttls'
  h.state.form.smtp.port = 587
  await h.state.save()
  assert.equal(h.state.form.smtp.security, 'starttls')
  assert.equal(h.state.form.smtp.port, 587)
})

test('SMTP defaults never replace saved settings or partially configured drafts', async (t) => {
  for (const patch of [
    { host: 'smtp.example.com' },
    { enabled: true },
    { username: 'login@example.com' },
    { passwordSet: true },
    { fromName: 'CPR' },
    { fromEmail: 'sender@example.com' },
    { security: 'none', port: 25 },
    { security: 'starttls', port: 2525 },
    { security: 'tls', port: 8465 },
  ]) {
    const h = channelHarness({ enabled: false, host: '', fromEmail: null, ...patch })
    t.after(h.stop)
    await h.state.load()
    assert.equal(h.state.form.smtp.security, patch.security ?? 'starttls')
    assert.equal(h.state.form.smtp.port, patch.port ?? 587)
    assert.equal(h.calls.saves.length, 0)
  }
})

for (const channel of ['email', 'bark']) {
  test(`${channel} test preserves both channel drafts after a rejected save and allows retry`, async (t) => {
    const h = channelHarness()
    t.after(h.stop)
    await h.state.load()
    Object.assign(h.state.form.smtp, { host: 'new-smtp.example.com', username: 'synthetic-user', password: 'synthetic-password' })
    Object.assign(h.state.form.bark, { serverUrl: 'https://new-push.example.com', deviceKey: 'synthetic-device-key' })
    h.state.testRecipient.value = 'ops@example.com'
    const draft = h.snapshot(h.state.form)
    h.failures.save = true

    await h.state.test(channel)

    assert.equal(h.calls.saves.length, 1)
    assert.equal(h.calls.tests.length, 0)
    assert.equal(h.calls.reads, 1)
    assert.deepEqual(h.snapshot(h.state.form), draft)
    assert.equal(h.state.testRecipient.value, 'ops@example.com')
    assert.equal(h.state.saving.value, false)
    assert.equal(h.state.testing.value, null)
    assert.deepEqual(h.errors, ['Synthetic save failure'])

    h.failures.save = false
    await h.state.test(channel)
    assert.equal(h.calls.saves.length, 2)
    assert.deepEqual(h.calls.saves[1], draft)
    assert.equal(h.calls.tests.length, 1)
    assert.equal(h.calls.tests[0].channel, channel)
    assert.equal(h.calls.tests[0].target, channel === 'email' ? 'ops@example.com' : 'default')
    assert.equal(h.calls.reads, 2)
    assert.equal(h.state.form.lastTest.status, 'sent')
    assert.equal(h.state.form.smtp.password, '')
    assert.equal(h.state.form.bark.deviceKey, '')
  })
}

test('delivery failure still reloads the saved test result without retrying the notification', async (t) => {
  const h = channelHarness()
  t.after(h.stop)
  await h.state.load()
  h.failures.send = true

  const pending = h.state.test('email')
  await h.state.test('bark')
  await pending

  assert.equal(h.calls.saves.length, 1)
  assert.equal(h.calls.tests.length, 1)
  assert.equal(h.calls.reads, 2)
  assert.equal(h.state.form.lastTest.status, 'failed')
  assert.equal(h.state.testing.value, null)
  assert.deepEqual(h.errors, ['Synthetic delivery failure'])
})

test('test result reload failure releases the busy state without resending', async (t) => {
  const h = channelHarness()
  t.after(h.stop)
  await h.state.load()
  h.failures.reload = true

  await h.state.test('bark')

  assert.equal(h.calls.saves.length, 1)
  assert.equal(h.calls.tests.length, 1)
  assert.equal(h.calls.reads, 2)
  assert.equal(h.state.loading.value, false)
  assert.equal(h.state.testing.value, null)
  assert.deepEqual(h.errors, ['Synthetic reload failure'])
})

test('notification API keeps channel, policy and test routes separate', async () => {
  const { api, calls } = loadApi()
  await api.getNotificationChannels()
  await api.updateNotificationChannels({ smtp: { enabled: false }, bark: { enabled: false } })
  await api.getGroupAlertPolicy('grp_demo')
  await api.updateGroupAlertPolicy({ groupId: 'grp_demo', updatedAt: 'readonly-time' })
  await api.testNotification({ channel: 'bark', target: 'default', groupId: 'grp_demo' })
  assert.deepEqual(calls.map(call => call.url), [
    '/api/admin/notifications/channels',
    '/api/admin/notifications/channels/update',
    '/api/admin/account-groups/alert-policy',
    '/api/admin/account-groups/alert-policy/update',
    '/api/admin/notifications/test',
  ])
  assert.equal(calls[2].params.groupId, 'grp_demo')
  assert.equal(calls[4].data.groupId, 'grp_demo')
  assert.equal(Object.hasOwn(calls[3].data, 'updatedAt'), false)
})

test('settings notifications are collapsed and independently testable', () => {
  const component = source('../src/views/settings/components/NotificationChannelsCard.vue')
  assert.match(component, /cpr\.settings\.notifications\.open/)
  assert.match(component, /cpr\.settings\.notifications\.smtp\.open/)
  assert.match(component, /cpr\.settings\.notifications\.bark\.open/)
  assert.match(component, /发送测试邮件/)
  assert.match(component, /发送测试通知/)
  assert.doesNotMatch(component, /\{\{\s*form\.smtp\.password\s*\}\}/)
  assert.doesNotMatch(component, /\{\{\s*form\.bark\.deviceKey\s*\}\}/)
})

test('notification forms share distinct Bark level, permission and ringing guidance', () => {
  const { barkLevelOptions, barkLevelHints } = loadPresentation()
  assert.deepEqual(Array.from(barkLevelOptions, option => option.value), ['active', 'timeSensitive', 'critical', 'passive'])
  assert.match(barkLevelOptions.find(option => option.value === 'critical').label, /静音仍响铃/)
  assert.match(barkLevelHints.critical, /开启「重要警告」/)
  assert.match(barkLevelHints.critical, /音量大于 0/)
  assert.match(barkLevelHints.timeSensitive, /仍受手机静音设置影响/)
  assert.match(barkLevelHints.passive, /不亮屏、不响铃/)
  for (const path of ['settings/components/NotificationChannelsCard.vue', 'accounts/components/GroupAlertSettingsModal.vue']) {
    const component = source(`../src/views/${path}`)
    assert.match(component, /import \{ barkLevelHints, barkLevelOptions \}/)
    assert.match(component, /持续响铃只延长时长，不会改变静音设置/)
  }
})

test('group card replaces monitor info with settings left of pin', () => {
  const card = source('../src/views/accounts/components/AccountGroupMonitorCard.vue')
  const settings = card.search(/设置 \$\{group\.name\} 预警/u)
  const pin = card.indexOf('`${pinned ? \'取消置顶\'')
  assert.ok(settings >= 0)
  assert.ok(pin > settings)
  assert.doesNotMatch(card, /查看监控口径/)
  const modal = source('../src/views/accounts/components/GroupAlertSettingsModal.vue')
  for (const text of ['并发达到阈值', '预计可支撑时间不足', '剩余额度为 0', '可调度账号为 0', '监控口径'])
    assert.match(modal, new RegExp(text))
  assert.match(modal, /预计过期额度仅供参考，不参与预警/)
})
