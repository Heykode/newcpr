import type { rotationOptions } from '../constants'
import type { RequestTuning } from '@/api/modules/settings'
import { computed, reactive, ref, shallowRef } from 'vue'

import { getSettings, updateSettings } from '@/api'
import { defaultSmartScheduling } from '@/api/modules/settings'
import { ApiError } from '@/api/request'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { errorMessage } from '@/utils/async'
import { DEFAULT_EXCEL_MODELS, DEFAULT_EXCEL_MODELS_INPUT } from '@/utils/excel-defaults'
import { parseExcelModels } from '@/views/accounts/utils/schedulingForm'

type RotationStrategy = (typeof rotationOptions)[number]['value']

const requestTuningFallbacks: RequestTuning = {
  smartScheduling: defaultSmartScheduling(),
  maxAccountSwitches: 31,
  maxRequestAttempts: 32,
  websocketMaxRetries: 5,
  websocketHttpFallbackEnabled: true,
  websocketLargeRequestThresholdBytes: 15 * 1024 * 1024,
  websocketMaxAgeMs: 55 * 60 * 1_000,
  websocketStreamIdleTimeoutMs: 300_000,
  websocketFailureThreshold: 3,
  websocketFailureWindowMs: 30_000,
  websocketFailureOpenDurationMs: 30_000,
  rateLimitCooldownSeconds: 60,
  excelImageRelayBytes: 1024 * 1024 * 1024,
  excelImageMaxBytes: 20 * 1024 * 1024,
  excelImageTotalBytes: 32 * 1024 * 1024,
  excelImageMaxCount: 20,
  excelImageRelayRequests: 128,
  excelImageRelayDownloads: 32,
  excelImageRelayEntries: 512,
  excelImageRelayTtlMinutes: 30,
  excelImageTransport: null,
  openaiLocationOverrideEnabled: false,
  openaiRequestLocation: null,
  maxWaitingPerKey: 0,
  keyConcurrencyWaitTimeoutSeconds: 30,
  accountBusyWaitEnabled: false,
  accountBusyWaitStickyMaxWaiting: 3,
  accountBusyWaitStickyTimeoutSeconds: 120,
  accountBusyWaitFallbackMaxWaiting: 100,
  accountBusyWaitFallbackTimeoutSeconds: 30,
}

export function useSettingsForm() {
  const loading = shallowRef(true)
  const saveAction = useAsyncAction()
  const saving = saveAction.loading
  const error = shallowRef('')
  const mappings = ref<Array<{ requestedModel: string, upstreamModel: string }>>([])
  const form = reactive({
    excelDefaultModels: DEFAULT_EXCEL_MODELS_INPUT,
    disableFast: false,
    responsesMaxDecompressedBodyBytes: 64 * 1024 * 1024,
    refreshMarginSeconds: null as number | null,
    refreshConcurrency: null as number | null,
    maxConcurrentPerAccount: null as number | null,
    requestIntervalMs: null as number | null,
    rotationStrategy: '' as RotationStrategy | '',
    minCodexDesktopVersion: '',
    minCodexCliVersion: '',
    usageRetentionDays: 31,
    opsEventRetentionDays: 30,
    auditRetentionDays: 90,
    requestTuning: { ...requestTuningFallbacks, smartScheduling: defaultSmartScheduling() },
  })

  function numericModel(key: 'refreshMarginSeconds' | 'refreshConcurrency' | 'maxConcurrentPerAccount' | 'requestIntervalMs') {
    return computed({
      get: () => (form[key] === null ? '' : String(form[key])),
      set: (value: string) => {
        if (!value.trim()) {
          form[key] = null
          return
        }
        const parsed = Number(value)
        form[key] = Number.isFinite(parsed) ? parsed : null
      },
    })
  }

  const refreshMarginSecondsValue = numericModel('refreshMarginSeconds')
  const refreshConcurrencyValue = numericModel('refreshConcurrency')
  const maxConcurrentPerAccountValue = numericModel('maxConcurrentPerAccount')
  const requestIntervalMsValue = numericModel('requestIntervalMs')
  const responsesMaxDecompressedBodyBytesValue = computed({
    get: () => String(form.responsesMaxDecompressedBodyBytes),
    set: (value: string) => {
      const parsed = Number(value)
      if (Number.isFinite(parsed))
        form.responsesMaxDecompressedBodyBytes = parsed
    },
  })
  const minCodexDesktopVersionError = computed(() => versionError(form.minCodexDesktopVersion))
  const minCodexCliVersionError = computed(() => versionError(form.minCodexCliVersion))

  function versionError(value: string): string {
    const normalized = value.trim()
    return normalized && !isSemver(normalized) ? '请输入标准 SemVer，例如 0.152.0' : ''
  }

  function applySettings(data: Awaited<ReturnType<typeof getSettings>>) {
    form.excelDefaultModels = (data.excelDefaultModels ?? DEFAULT_EXCEL_MODELS).join(', ')
    form.disableFast = data.disableFast ?? false
    form.responsesMaxDecompressedBodyBytes
      = data.responsesMaxDecompressedBodyBytes ?? 64 * 1024 * 1024
    form.refreshMarginSeconds = data.refreshMarginSeconds
    form.refreshConcurrency = data.refreshConcurrency
    form.maxConcurrentPerAccount = data.maxConcurrentPerAccount
    form.requestIntervalMs = data.requestIntervalMs
    form.rotationStrategy = data.rotationStrategy
    form.minCodexDesktopVersion = data.minCodexDesktopVersion ?? ''
    form.minCodexCliVersion = data.minCodexCliVersion ?? ''
    form.usageRetentionDays = data.usageRetentionDays
    form.opsEventRetentionDays = data.opsEventRetentionDays
    form.auditRetentionDays = data.auditRetentionDays
    const tuning = data.requestTuning ?? {}
    const defaults = data.requestTuningDefaults ?? {}
    Object.assign(
      form.requestTuning,
      Object.fromEntries(
        (Object.keys(requestTuningFallbacks) as Array<keyof RequestTuning>).map(key => [
          key,
          tuning[key] ?? defaults[key] ?? requestTuningFallbacks[key],
        ]),
      ),
    )
    form.requestTuning.smartScheduling = { ...form.requestTuning.smartScheduling }
    mappings.value = Object.entries(data.modelMappings || {}).map(([requestedModel, upstreamModel]) => ({
      requestedModel,
      upstreamModel: String(upstreamModel),
    }))
  }

  async function loadSettings(silent = false) {
    loading.value = true
    error.value = ''
    try {
      applySettings(await getSettings({ silent }))
    }
    catch (cause: unknown) {
      error.value = errorMessage(cause)
    }
    finally {
      loading.value = false
    }
  }

  function addMapping() {
    mappings.value = [...mappings.value, { requestedModel: '', upstreamModel: '' }]
  }

  function updateMapping(index: number, key: 'requestedModel' | 'upstreamModel', value: string) {
    const rows = [...mappings.value]
    if (!rows[index])
      return
    rows[index] = { ...rows[index], [key]: value }
    mappings.value = rows
  }

  function removeMapping(index: number) {
    const rows = [...mappings.value]
    rows.splice(index, 1)
    mappings.value = rows
  }

  function mappingPayload() {
    const entries: Record<string, string> = {}
    for (const row of mappings.value) {
      const requested = row.requestedModel.trim()
      const upstream = row.upstreamModel.trim()
      if (!requested || !upstream)
        throw new Error('请完整填写模型映射')
      if (entries[requested])
        throw new Error(`存在重复的客户端模型：${requested}`)
      entries[requested] = upstream
    }
    return entries
  }

  async function saveSettings() {
    if (saving.value || loading.value)
      return
    const excelDefaultModels = parseExcelModels(form.excelDefaultModels)
    if (excelDefaultModels === null) {
      toast.warning('Excel 模型名称不合法，或超过 64 个')
      return
    }
    const { refreshMarginSeconds, refreshConcurrency, maxConcurrentPerAccount, requestIntervalMs, rotationStrategy } = form
    if (refreshMarginSeconds === null || refreshConcurrency === null || maxConcurrentPerAccount === null || requestIntervalMs === null || !rotationStrategy) {
      toast.warning('请完整填写运行参数和调度策略')
      return
    }
    if (minCodexDesktopVersionError.value || minCodexCliVersionError.value) {
      toast.warning('请修正客户端最低版本格式')
      return
    }
    const tuning = form.requestTuning
    const imageTransport = tuning.excelImageTransport
    if (imageTransport?.mode === 'relay') {
      const origin = imageTransport.publicUrl.trim()
      let valid = false
      try {
        const parsed = new URL(origin)
        const invalidCharacter = [...origin].some(character => character === '\\' || character.charCodeAt(0) <= 32 || character.charCodeAt(0) === 127)
        valid = origin.length <= 2048 && !invalidCharacter
          && parsed.protocol === 'https:' && !parsed.username && !parsed.password
          && !origin.includes('?') && !origin.includes('#') && parsed.pathname === '/'
          && parsed.hostname.includes('.') && !/^[\d.]+$/u.test(parsed.hostname)
          && !parsed.hostname.endsWith('.localhost') && !parsed.hostname.endsWith('.local')
      }
      catch { /* Invalid origins are rejected before saving. */ }
      if (!valid) {
        toast.warning('HTTPS 中转需要有效的公网域名，不得包含路径、账号密码、查询参数或片段')
        return
      }
      tuning.excelImageTransport = { mode: 'relay', publicUrl: origin }
    }
    const smartWeights = [
      tuning.smartScheduling.loadWeight,
      tuning.smartScheduling.quotaWeight,
      tuning.smartScheduling.healthWeight,
      tuning.smartScheduling.latencyWeight,
      tuning.smartScheduling.resetWeight,
      tuning.smartScheduling.queueWeight,
    ]
    if (!smartWeights.every(value => Number.isFinite(value) && value >= 0 && value <= 10 && Math.round(value * 10) / 10 === value)
      || !smartWeights.some(value => value > 0)) {
      toast.warning('智能调度权重须为 0–10，最多一位小数，且不能全部为 0')
      return
    }
    if (!Number.isInteger(tuning.excelImageMaxBytes) || tuning.excelImageMaxBytes < 1 || tuning.excelImageMaxBytes > 128 * 1024 * 1024
      || !Number.isInteger(tuning.excelImageTotalBytes) || tuning.excelImageTotalBytes < 1 || tuning.excelImageTotalBytes > 128 * 1024 * 1024
      || !Number.isInteger(tuning.excelImageMaxCount) || tuning.excelImageMaxCount < 1 || tuning.excelImageMaxCount > 4096) {
      toast.warning('单张图片和请求图片总量须为 1–134217728 字节，图片数量须为 1–4096')
      return
    }
    if (!Number.isInteger(tuning.excelImageRelayBytes) || tuning.excelImageRelayBytes < 1024 * 1024 || tuning.excelImageRelayBytes > 16384 * 1024 * 1024
      || !Number.isInteger(tuning.excelImageRelayRequests) || tuning.excelImageRelayRequests < 1 || tuning.excelImageRelayRequests > 512
      || !Number.isInteger(tuning.excelImageRelayDownloads) || tuning.excelImageRelayDownloads < 1 || tuning.excelImageRelayDownloads > 128
      || !Number.isInteger(tuning.excelImageRelayEntries) || tuning.excelImageRelayEntries < 1 || tuning.excelImageRelayEntries > 65536
      || !Number.isInteger(tuning.excelImageRelayTtlMinutes) || tuning.excelImageRelayTtlMinutes < 1 || tuning.excelImageRelayTtlMinutes > 1440) {
      toast.warning('图片预算须为 1–16384 MiB，下载并发须为 1–128，条目须为 1–65536，有效期须为 1–1440 分钟')
      return
    }
    if (tuning.excelImageTotalBytes < tuning.excelImageMaxBytes
      || tuning.excelImageRelayBytes < tuning.excelImageTotalBytes
      || tuning.excelImageRelayEntries < tuning.excelImageMaxCount) {
      toast.warning('图片总量不得小于单张上限，中转预算不得小于总量，条目不得小于图片数量')
      return
    }
    if (!Number.isInteger(form.responsesMaxDecompressedBodyBytes)
      || form.responsesMaxDecompressedBodyBytes < 1
      || form.responsesMaxDecompressedBodyBytes > 256 * 1024 * 1024) {
      toast.warning('请求解压上限须为 1–268435456 的整数字节数')
      return
    }
    if (!Number.isInteger(tuning.websocketLargeRequestThresholdBytes)
      || tuning.websocketLargeRequestThresholdBytes < 0
      || tuning.websocketLargeRequestThresholdBytes > 64 * 1024 * 1024) {
      toast.warning('大请求 HTTP 阈值须为 0–67108864 的整数字节数')
      return
    }
    if (!Number.isInteger(tuning.maxWaitingPerKey) || tuning.maxWaitingPerKey < 0 || tuning.maxWaitingPerKey > 1024
      || !Number.isInteger(tuning.keyConcurrencyWaitTimeoutSeconds) || tuning.keyConcurrencyWaitTimeoutSeconds < 1 || tuning.keyConcurrencyWaitTimeoutSeconds > 600) {
      toast.warning('Key 等待人数须为 0–1024 的整数，等待秒数须为 1–600 的整数')
      return
    }
    if (![tuning.accountBusyWaitStickyMaxWaiting, tuning.accountBusyWaitFallbackMaxWaiting]
      .every(value => Number.isInteger(value) && value >= 1 && value <= 1000)
      || ![tuning.accountBusyWaitStickyTimeoutSeconds, tuning.accountBusyWaitFallbackTimeoutSeconds]
        .every(value => Number.isInteger(value) && value >= 1 && value <= 600)) {
      toast.warning('OpenAI 账号忙时等待人数须为 1–1000 的整数，等待秒数须为 1–600 的整数')
      return
    }
    await saveAction.run(async () => {
      const result = await updateSettings({
        excelDefaultModels,
        disableFast: form.disableFast,
        responsesMaxDecompressedBodyBytes: form.responsesMaxDecompressedBodyBytes,
        modelMappings: mappingPayload(),
        refreshMarginSeconds,
        refreshConcurrency,
        maxConcurrentPerAccount,
        requestIntervalMs,
        rotationStrategy,
        minCodexDesktopVersion: form.minCodexDesktopVersion.trim() || null,
        minCodexCliVersion: form.minCodexCliVersion.trim() || null,
        usageRetentionDays: form.usageRetentionDays,
        opsEventRetentionDays: form.opsEventRetentionDays,
        auditRetentionDays: form.auditRetentionDays,
        requestTuning: {
          ...form.requestTuning,
          excelImageTransport: form.requestTuning.excelImageTransport
            ? { ...form.requestTuning.excelImageTransport }
            : null,
          smartScheduling: { ...form.requestTuning.smartScheduling },
          openaiRequestLocation: form.requestTuning.openaiRequestLocation
            ? { ...form.requestTuning.openaiRequestLocation }
            : null,
        },
      })
      applySettings(result)
      toast.success('设置已保存')
    }, {
      onError: (cause) => {
        if (cause instanceof ApiError)
          void loadSettings(true)
      },
    })
  }

  return {
    loading,
    saving,
    error,
    form,
    mappings,
    addMapping,
    updateMapping,
    removeMapping,
    refreshMarginSecondsValue,
    refreshConcurrencyValue,
    maxConcurrentPerAccountValue,
    requestIntervalMsValue,
    responsesMaxDecompressedBodyBytesValue,
    minCodexDesktopVersionError,
    minCodexCliVersionError,
    saveSettings,
    loadSettings,
  }
}

function isSemver(value: string): boolean {
  if (value.length > 64 || value.startsWith('v'))
    return false

  const buildParts = value.split('+')
  if (buildParts.length > 2)
    return false
  const [versionAndPrerelease = '', build] = buildParts
  if (build !== undefined && !validIdentifiers(build, false))
    return false

  const prereleaseSeparator = versionAndPrerelease.indexOf('-')
  const core = prereleaseSeparator < 0
    ? versionAndPrerelease
    : versionAndPrerelease.slice(0, prereleaseSeparator)
  const prerelease = prereleaseSeparator < 0
    ? undefined
    : versionAndPrerelease.slice(prereleaseSeparator + 1)
  if (prerelease !== undefined && !validIdentifiers(prerelease, true))
    return false

  const coreParts = core.split('.')
  return coreParts.length === 3 && coreParts.every(validCoreNumericIdentifier)
}

function validIdentifiers(value: string, rejectNumericLeadingZeros: boolean): boolean {
  return Boolean(value) && value.split('.').every((identifier) => {
    if (!identifier || !/^[\da-z-]+$/i.test(identifier))
      return false
    return !rejectNumericLeadingZeros || !/^\d+$/.test(identifier) || validNumericIdentifier(identifier)
  })
}

function validNumericIdentifier(value: string): boolean {
  return /^(?:0|[1-9]\d*)$/.test(value)
}

function validCoreNumericIdentifier(value: string): boolean {
  return validNumericIdentifier(value) && BigInt(value) <= 18_446_744_073_709_551_615n
}
