import type { Account, AccountModelsResponse } from '@/api'
import { CheckCircle2, Clock3, Wifi, XCircle } from '@lucide/vue'

import { useLocalStorage } from '@vueuse/core'
import { clamp } from 'es-toolkit'
import { computed, onBeforeUnmount, ref, shallowRef, watch } from 'vue'
import { getAccountModels, refreshAccountModels } from '@/api'
import { streamAccountConnectionTest } from '@/api/modules/account-connection-test'
import { toast } from '@/components/base/BaseToast'
import { useIdSet } from '@/composables/useIdSet'
import { errorMessage, withMinimumDuration } from '@/utils/async'
import { formatDateTime, formatTime } from '@/utils/date'

interface ConnectionTestRun {
  accountId: string
  controller: AbortController
}

interface ConnectionTestSettings {
  endpoint: 'responses'
  stream: boolean
  prompt: string
}

const DEFAULT_CONNECTION_TEST_SETTINGS: ConnectionTestSettings = {
  endpoint: 'responses',
  stream: true,
  prompt: 'Reply with exactly OK.',
}

function normalizeConnectionTestSettings(value: unknown): ConnectionTestSettings {
  const stored = value && typeof value === 'object' && !Array.isArray(value)
    ? value as Partial<ConnectionTestSettings>
    : {}
  return {
    endpoint: 'responses',
    stream: stored.stream !== false,
    prompt: typeof stored.prompt === 'string' ? stored.prompt : DEFAULT_CONNECTION_TEST_SETTINGS.prompt,
  }
}

type ConnectionTestStatus = 'idle' | 'running' | 'success' | 'error'
type ConnectionTestLogTone = 'normal' | 'info' | 'success' | 'danger'

interface ConnectionTestModelOption {
  label: string
  value: string
}

interface ConnectionTestLog {
  key: string
  time: string
  text: string
  tone: ConnectionTestLogTone
  detail: string
}

interface ConnectionTestRequestPayload {
  input?: Array<{ content?: Array<{ type?: string, text?: string }> }>
  prompt?: string
}

interface ConnectionTestStartEvent {
  type: 'test_start'
  text?: string
  model?: string
}

interface ConnectionTestRequestEvent {
  type: 'request'
  payload?: ConnectionTestRequestPayload
}

interface ConnectionTestStatusEvent {
  type: 'status'
  text?: string
}

interface ConnectionTestContentEvent {
  type: 'content'
  text?: string
}

interface ConnectionTestCompleteEvent {
  type: 'test_complete'
  success: boolean
  error?: string
}

type ConnectionTestFailureSource = 'gateway' | 'provider' | 'upstream'
type ConnectionTestSendState = 'not_sent' | 'sent' | 'ambiguous'

interface ConnectionTestFailureEvent {
  type: 'error'
  source?: ConnectionTestFailureSource
  gatewayErrorCode?: string
  sendState?: ConnectionTestSendState | null
  error?: string
  providerErrorCode?: string | null
  providerErrorType?: string | null
  upstreamStatus?: number | null
  upstreamContentType?: string | null
  upstreamBody?: string | null
}

type ConnectionTestEvent
  = | ConnectionTestStartEvent
    | ConnectionTestRequestEvent
    | ConnectionTestStatusEvent
    | ConnectionTestContentEvent
    | ConnectionTestCompleteEvent
    | ConnectionTestFailureEvent

const CONNECTION_TEST_EVENT_TYPES = new Set<ConnectionTestEvent['type']>([
  'test_start',
  'request',
  'status',
  'content',
  'test_complete',
  'error',
])

const CONNECTION_TEST_FAILURE_TEXT: Record<string, string> = {
  invalid_request: '测试请求不合法',
  unsupported: '当前 Provider 不支持连接测试',
  unauthorized: '账号登录已失效或被上游拒绝，请先刷新 Token；仍失败再重新授权',
  policy_denied: '测试请求被网关策略拒绝',
  model_not_found: '测试模型不存在',
  no_available_provider: '指定账号当前不可用于连接测试',
  account_capacity_unavailable: '指定账号当前没有可用容量',
  provider_infrastructure_unavailable: 'Provider 本地基础设施暂不可用',
  rate_limited: '账号触发了上游限流，请稍后重试',
  upstream_unavailable: '上游服务暂时不可用，请稍后重试',
  timeout: '上游请求超时',
  cancelled: '测试请求已取消',
  internal_error: '网关内部错误',
}

const CONNECTION_TEST_SOURCE_LABEL: Record<ConnectionTestFailureSource, string> = {
  gateway: '网关校验',
  provider: 'Provider 本地准备',
  upstream: '上游响应',
}

function parseConnectionTestEvent(raw: string): ConnectionTestEvent | null {
  const value: unknown = JSON.parse(raw)
  if (!value || typeof value !== 'object' || !('type' in value) || typeof value.type !== 'string')
    throw new TypeError('invalid connection-test event')
  if (!CONNECTION_TEST_EVENT_TYPES.has(value.type as ConnectionTestEvent['type']))
    return null
  return value as ConnectionTestEvent
}

function connectionTestFailureText(event: ConnectionTestFailureEvent) {
  if (event.upstreamStatus === 401 || event.upstreamStatus === 403) {
    return '账号登录已失效或被上游拒绝，请先刷新 Token；仍失败再重新授权'
  }
  if (event.upstreamStatus === 429) {
    return '账号触发了上游限流，请稍后重试'
  }
  if (
    event.upstreamStatus !== undefined
    && event.upstreamStatus !== null
    && event.upstreamStatus >= 500
  ) {
    return '上游服务暂时不可用，请稍后重试'
  }
  return event.gatewayErrorCode
    ? CONNECTION_TEST_FAILURE_TEXT[event.gatewayErrorCode] || '未分类错误'
    : '测试连接失败'
}

function connectionTestFailureLabel(event: ConnectionTestFailureEvent) {
  return event.source ? CONNECTION_TEST_SOURCE_LABEL[event.source] || '测试失败' : '测试失败'
}

function connectionTestFailureDiagnostics(event: ConnectionTestFailureEvent) {
  return {
    error: event.error ?? null,
    gatewayErrorCode: event.gatewayErrorCode ?? null,
    sendState: event.sendState ?? null,
    upstreamStatus: event.upstreamStatus ?? null,
    providerErrorCode: event.providerErrorCode ?? null,
    providerErrorType: event.providerErrorType ?? null,
    upstreamContentType: event.upstreamContentType ?? null,
    upstreamBody: event.upstreamBody ?? null,
  }
}

export function useAccountConnectionTest(options: { reload: () => Promise<unknown> }) {
  const showConnectionTestModal = shallowRef(false)
  const testingAccount = shallowRef<Account | null>(null)
  const connectionTestStatus = shallowRef<ConnectionTestStatus>('idle')
  const connectionTestModel = shallowRef('')
  const connectionTestContent = shallowRef('')
  const connectionTestLogs = ref<ConnectionTestLog[]>([])
  const connectionTestError = shallowRef('')
  const connectionTestStartedAt = shallowRef('')
  const connectionTestFinishedAt = shallowRef('')
  const connectionTestDurationMs = shallowRef<number | null>(null)
  const testingConnections = useIdSet<string>()
  const loadingConnectionTestModels = shallowRef(false)
  const refreshingConnectionTestModels = shallowRef(false)
  const connectionTestSelectedModel = shallowRef('')
  const connectionTestSettings = useLocalStorage<ConnectionTestSettings>(
    'cpr.accounts.connection-test-settings',
    { ...DEFAULT_CONNECTION_TEST_SETTINGS },
    {
      serializer: {
        read: (raw) => {
          try {
            return normalizeConnectionTestSettings(JSON.parse(raw))
          }
          catch {
            return { ...DEFAULT_CONNECTION_TEST_SETTINGS }
          }
        },
        write: value => JSON.stringify(normalizeConnectionTestSettings(value)),
      },
    },
  )
  const connectionTestModelOptions = ref<ConnectionTestModelOption[]>([])

  let connectionTestStartedAtMs = 0
  let connectionTestRun: ConnectionTestRun | undefined
  let connectionTestGeneration = 0
  let connectionTestModelRequest = 0

  const connectionTestStatusView = computed(() => {
    if (connectionTestStatus.value === 'running') {
      return {
        label: '正在测试',
        description: '正在向所选模型发送请求并接收响应',
        icon: Clock3,
        badge: 'bg-cp-info-container text-cp-info-on-container',
        iconClass: 'text-cp-info',
      }
    }
    if (connectionTestStatus.value === 'success') {
      return {
        label: '连接正常',
        description: '请求已完成，可在下方查看模型、耗时和事件轨迹',
        icon: CheckCircle2,
        badge: 'bg-cp-success-container text-cp-success-on-container',
        iconClass: 'text-cp-success',
      }
    }
    if (connectionTestStatus.value === 'error') {
      return {
        label: '测试失败',
        description: '请求未完成，请在下方查看失败来源与原始诊断',
        icon: XCircle,
        badge: 'bg-cp-error-container text-cp-error-on-container',
        iconClass: 'text-cp-error',
      }
    }
    return {
      label: '准备测试',
      description: '选择模型后，点击“开始测试”发送真实请求',
      icon: Wifi,
      badge: 'bg-cp-fill-quaternary text-cp-text-secondary',
      iconClass: 'text-cp-text-quaternary',
    }
  })

  function openConnectionTest(account: Account) {
    abortConnectionTest()
    testingAccount.value = account
    connectionTestSelectedModel.value = ''
    connectionTestModelOptions.value = []
    refreshingConnectionTestModels.value = false
    showConnectionTestModal.value = true
    resetConnectionTest()
    void loadConnectionTestModels(account)
  }

  function resetConnectionTest() {
    connectionTestStatus.value = 'idle'
    connectionTestModel.value = ''
    connectionTestContent.value = ''
    connectionTestLogs.value = []
    connectionTestError.value = ''
    connectionTestStartedAt.value = ''
    connectionTestFinishedAt.value = ''
    connectionTestDurationMs.value = null
    connectionTestStartedAtMs = 0
  }

  function connectionTestPromptByteLength(prompt: string) {
    return new TextEncoder().encode(prompt).byteLength
  }

  function formatConnectionTestDetail(value: unknown) {
    if (value === undefined || value === null || value === '')
      return ''
    if (typeof value === 'string')
      return value
    return JSON.stringify(value, null, 2)
  }

  function connectionTestRequestText(payload?: ConnectionTestRequestPayload) {
    const texts = (payload?.input ?? [])
      .flatMap(item => item.content ?? [])
      .filter(item => item.type === 'input_text' && item.text)
      .map(item => item.text)
    return texts.join('\n') || payload?.prompt || ''
  }

  function connectionTestLogItem(
    key: string,
    text: string,
    tone: ConnectionTestLogTone = 'normal',
    detail?: unknown,
  ): ConnectionTestLog {
    return {
      key,
      time: formatTime(),
      text,
      tone,
      detail: formatConnectionTestDetail(detail),
    }
  }

  function appendConnectionTestLog(
    text: string,
    tone: ConnectionTestLogTone = 'normal',
    detail?: unknown,
  ) {
    connectionTestLogs.value = [
      ...connectionTestLogs.value,
      connectionTestLogItem(`${Date.now()}-${connectionTestLogs.value.length}`, text, tone, detail),
    ]
  }

  function setConnectionTestLog(
    key: string,
    text: string,
    tone: ConnectionTestLogTone = 'normal',
    detail?: unknown,
  ) {
    const index = connectionTestLogs.value.findIndex(item => item.key === key)
    const next = connectionTestLogItem(key, text, tone, detail)
    if (index === -1) {
      connectionTestLogs.value = [...connectionTestLogs.value, next]
      return
    }
    connectionTestLogs.value = connectionTestLogs.value.map((item, itemIndex) =>
      itemIndex === index ? { ...next, time: item.time } : item,
    )
  }

  function finishConnectionTest(status: 'success' | 'error') {
    connectionTestStatus.value = status
    connectionTestFinishedAt.value = formatDateTime()
    connectionTestDurationMs.value = clamp(
      Date.now() - connectionTestStartedAtMs,
      0,
      Number.POSITIVE_INFINITY,
    )
  }

  function clearConnectionTestRun() {
    const run = connectionTestRun
    connectionTestRun = undefined
    if (run) {
      run.controller.abort()
      testingConnections.remove(run.accountId)
    }
  }

  function failConnectionTest(message = '测试连接失败') {
    if (connectionTestStatus.value === 'running') {
      recordConnectionTestFailure('failure', '测试失败', message)
    }
    clearConnectionTestRun()
  }

  function recordConnectionTestFailure(
    key: string,
    label: string,
    message: string,
    detail?: unknown,
  ) {
    connectionTestError.value = message
    setConnectionTestLog(key, `${label}：${message}`, 'danger', detail)
    finishConnectionTest('error')
  }

  function handleConnectionTestEvent(event: ConnectionTestEvent) {
    if (event.type === 'test_start') {
      connectionTestModel.value = event.model || connectionTestModel.value
      appendConnectionTestLog(`开始测试 ${connectionTestModel.value || '未选择模型'}`, 'info')
      return
    }
    if (event.type === 'request') {
      setConnectionTestLog('request', '发起请求', 'info', connectionTestRequestText(event.payload))
      return
    }
    if (event.type === 'status' && event.text) {
      appendConnectionTestLog(event.text, 'info')
      return
    }
    if (event.type === 'content' && event.text) {
      connectionTestContent.value += event.text
      setConnectionTestLog('response', '接收响应内容', 'success', connectionTestContent.value)
      return
    }
    if (event.type === 'test_complete') {
      if (event.success) {
        if (!connectionTestContent.value) {
          setConnectionTestLog('response', '响应完成', 'success', '上游已完成，没有返回文本内容')
        }
        appendConnectionTestLog('测试完成', 'success')
        finishConnectionTest('success')
      }
      else {
        recordConnectionTestFailure(
          'test-complete-failure',
          '测试失败',
          '测试连接失败',
          { error: event.error ?? null },
        )
      }
      clearConnectionTestRun()
      void options.reload()
      return
    }
    if (event.type === 'error') {
      recordConnectionTestFailure(
        `failure-${event.source || 'unknown'}`,
        connectionTestFailureLabel(event),
        connectionTestFailureText(event),
        connectionTestFailureDiagnostics(event),
      )
      clearConnectionTestRun()
      void options.reload()
    }
  }

  function abortConnectionTest() {
    connectionTestGeneration += 1
    clearConnectionTestRun()
  }

  async function loadConnectionTestModels(account = testingAccount.value) {
    if (!account?.id)
      return
    const request = ++connectionTestModelRequest
    loadingConnectionTestModels.value = true
    connectionTestError.value = ''
    try {
      const result = await getAccountModels({ accountId: account.id }, { silent: true })
      if (request !== connectionTestModelRequest)
        return
      applyConnectionTestModels(result)
      if (!connectionTestSelectedModel.value) {
        connectionTestError.value = '没有可测试模型'
      }
    }
    catch (error: unknown) {
      if (request !== connectionTestModelRequest)
        return
      connectionTestError.value
        = `模型列表加载失败：${errorMessage(error, '无法读取上游模型，请稍后重试')}`
      connectionTestModelOptions.value = []
      connectionTestSelectedModel.value = ''
    }
    finally {
      if (request === connectionTestModelRequest)
        loadingConnectionTestModels.value = false
    }
  }

  function applyConnectionTestModels(result: AccountModelsResponse, preserveSelection = false) {
    const previousSelection = preserveSelection ? connectionTestSelectedModel.value : ''
    connectionTestModelOptions.value = []
    for (const model of result.models ?? []) {
      connectionTestModelOptions.value.push({
        label: model.label || model.id,
        value: model.id,
      })
    }
    connectionTestSelectedModel.value = connectionTestModelOptions.value.some(
      model => model.value === previousSelection,
    )
      ? previousSelection
      : connectionTestModelOptions.value[0]?.value || ''
  }

  async function handleRefreshConnectionTestModels(account = testingAccount.value) {
    if (!account?.id || refreshingConnectionTestModels.value)
      return
    const request = ++connectionTestModelRequest
    refreshingConnectionTestModels.value = true
    connectionTestError.value = ''
    try {
      const result = await refreshAccountModels({ accountId: account.id }, { silent: true })
      if (request !== connectionTestModelRequest)
        return
      applyConnectionTestModels(result, true)
      toast.success(`已刷新 ${connectionTestModelOptions.value.length} 个上游模型`)
    }
    catch (error: unknown) {
      if (request !== connectionTestModelRequest)
        return
      connectionTestError.value
        = `模型列表刷新失败：${errorMessage(error, '无法读取上游模型，请稍后重试')}`
      toast.error(connectionTestError.value)
    }
    finally {
      if (request === connectionTestModelRequest)
        refreshingConnectionTestModels.value = false
    }
  }

  async function handleTestConnection(account = testingAccount.value) {
    if (!account?.id)
      return
    if (!connectionTestSelectedModel.value) {
      connectionTestError.value = '请先选择测试模型'
      return
    }
    const settings = connectionTestSettings.value
    if (!settings.prompt.trim()) {
      connectionTestError.value = '请先填写测试提示词'
      return
    }
    if (connectionTestPromptByteLength(settings.prompt) > 4096) {
      connectionTestError.value = '测试提示词不能超过 4096 字节'
      return
    }
    if (Array.from(settings.prompt).some((character) => {
      const code = character.codePointAt(0)!
      return (code < 32 || (code >= 127 && code <= 159)) && !['\n', '\r', '\t'].includes(character)
    })) {
      connectionTestError.value = '测试提示词不能包含不可见控制字符'
      return
    }
    if (testingConnections.has(account.id))
      return
    abortConnectionTest()
    const generation = connectionTestGeneration
    connectionTestStatus.value = 'running'
    connectionTestModel.value = ''
    connectionTestContent.value = ''
    connectionTestLogs.value = []
    connectionTestError.value = ''
    connectionTestDurationMs.value = null
    connectionTestModel.value = connectionTestSelectedModel.value
    connectionTestStartedAtMs = Date.now()
    connectionTestStartedAt.value = formatDateTime()
    connectionTestFinishedAt.value = ''
    appendConnectionTestLog('准备发送测试请求', 'info')
    testingConnections.add(account.id)
    const run: ConnectionTestRun = {
      accountId: account.id,
      controller: new AbortController(),
    }
    connectionTestRun = run
    try {
      await withMinimumDuration(async () => {
        try {
          await streamAccountConnectionTest({
            accountId: account.id,
            modelId: connectionTestSelectedModel.value,
            interface: settings.endpoint,
            stream: settings.stream,
            prompt: settings.prompt,
          }, run.controller.signal, (raw) => {
            if (generation !== connectionTestGeneration || connectionTestRun !== run)
              return
            try {
              const event = parseConnectionTestEvent(raw)
              if (event)
                handleConnectionTestEvent(event)
            }
            catch {
              failConnectionTest('测试响应解析失败')
            }
          })
        }
        catch (error: unknown) {
          if (!run.controller.signal.aborted)
            throw error
        }
      })
      if (generation === connectionTestGeneration && connectionTestStatus.value === 'running') {
        recordConnectionTestFailure('failure', '测试失败', '测试连接未返回完成事件')
      }
    }
    catch (error: unknown) {
      if (generation === connectionTestGeneration)
        recordConnectionTestFailure('failure', '测试失败', errorMessage(error, '测试连接失败'))
    }
    finally {
      if (generation === connectionTestGeneration)
        clearConnectionTestRun()
    }
  }

  watch(showConnectionTestModal, (open) => {
    if (!open) {
      connectionTestModelRequest += 1
      abortConnectionTest()
    }
  })

  onBeforeUnmount(() => {
    connectionTestModelRequest += 1
    abortConnectionTest()
  })

  return {
    showConnectionTestModal,
    testingAccount,
    connectionTestStatus,
    connectionTestModel,
    connectionTestLogs,
    connectionTestError,
    connectionTestStartedAt,
    connectionTestFinishedAt,
    connectionTestDurationMs,
    testingConnectionIds: testingConnections.ids,
    loadingConnectionTestModels,
    refreshingConnectionTestModels,
    connectionTestSelectedModel,
    connectionTestStream: computed({
      get: () => connectionTestSettings.value.stream,
      set: (value) => {
        connectionTestSettings.value = {
          ...connectionTestSettings.value,
          stream: value,
        }
      },
    }),
    connectionTestPrompt: computed({
      get: () => connectionTestSettings.value.prompt,
      set: (value) => {
        connectionTestSettings.value = {
          ...connectionTestSettings.value,
          prompt: value,
        }
      },
    }),
    connectionTestModelOptions,
    connectionTestStatusView,
    openConnectionTest,
    handleRefreshConnectionTestModels,
    handleTestConnection,
  }
}
