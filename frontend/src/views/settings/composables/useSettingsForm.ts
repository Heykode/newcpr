import type { ProtectionDraft } from '../components/error-protection/form'
import type { rotationOptions } from '../constants'
import type { CodexPrivacyPolicy, ProviderRequestProfiles } from '@/api'
import type { AccountAffinity, RequestTuning } from '@/api/modules/settings'
import { cloneDeep } from 'es-toolkit'
import { computed, reactive, ref, shallowRef } from 'vue'
import { getSettings, updateSettings } from '@/api'
import { defaultRequestLocation } from '@/api/modules/proxies'
import { defaultOpenAiRequestSettings, defaultOpenAiWebSocketPool, defaultResetWindowWeights, defaultSmartScheduling } from '@/api/modules/settings'
import { ApiError } from '@/api/request'
import { toast } from '@/components/base/BaseToast'

import { requestProfileChanges } from '@/components/request-profile/changes'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { t } from '@/i18n'
import { errorMessage } from '@/utils/async'
import { normalizeRequestLocation, requestLocationError } from '@/utils/location'
import { cloneProtectionRules, protectionSettingsError, trimProtectionText } from '../components/error-protection/form'
import { validWarmupExpressions, warmupScheduleBytes } from '../utils/warmup-schedule'
import { useRuntimeUnits } from './useRuntimeUnits'

function defaultProtection(): ProtectionDraft {
  return {
    enabled: false,
    rules: [],
    threshold: 12,
    windowSeconds: 600,
    durationSeconds: 7200,
    probeEnabled: true,
    probeModel: '',
  }
}

type RotationStrategy = (typeof rotationOptions)[number]['value']

const requestTuningFallbacks: RequestTuning = {
  smartScheduling: defaultSmartScheduling(),
  maxAccountSwitches: 31,
  maxRequestAttempts: 32,
  websocketMaxRetries: 5,
  streamPrefetchBytes: 128 * 1024,
  websocketFailureThreshold: 3,
  websocketFailureWindowMs: 30_000,
  websocketFailureOpenDurationMs: 30_000,
  rateLimitCooldownSeconds: 60,
  maxRetryAfterSeconds: 240,
  openaiLocationOverrideEnabled: false,
  openaiRequestLocation: null,
  maxWaitingPerKey: 0,
  keyQueueTimeoutSeconds: 30,
  accountBusyWaitEnabled: false,
  switchAccountOnQueueFailure: false,
  maxWaitingPerAccount: 10,
  accountQueueTimeoutSeconds: 30,
}

export function useSettingsForm() {
  const loading = shallowRef(true)
  const saveAction = useAsyncAction()
  const saving = saveAction.loading
  const error = shallowRef('')
  const configRevision = shallowRef(0)
  const mappings = ref<Array<{ requestedModel: string, upstreamModel: string }>>([])
  const savedProfiles = shallowRef<ProviderRequestProfiles>({})
  const form = reactive({
    codexPrivacyPolicy: { enabled: false, onError: 'skip_rule', rules: [] } as CodexPrivacyPolicy,
    providerRequestProfiles: {} as ProviderRequestProfiles,
    openaiRequestSettings: defaultOpenAiRequestSettings(),
    openaiWebsocketPool: defaultOpenAiWebSocketPool(),
    accountErrorProtection: defaultProtection(),
    responsesMaxDecompressedBodyBytes: 64 * 1024 * 1024,
    accountAffinity: 'strict' as AccountAffinity,
    refreshMarginSeconds: null as number | null,
    refreshConcurrency: null as number | null,
    maxConcurrentPerAccount: null as number | null,
    openaiGuardianReservedConcurrency: null as number | null,
    accountWarmupEnabled: false,
    accountWarmupCron: [] as string[],
    accountWarmupModel: '',
    codexQuotaOverdraftEnabled: false,
    codexQuotaOverdraftProbeModel: 'gpt-6-astra',
    requestIntervalMs: null as number | null,
    sessionAffinityTtlHours: null as number | null,
    rotationStrategy: '' as RotationStrategy | '',
    quotaResetWindowWeights: defaultResetWindowWeights(),
    quotaResetWindowWeightsDefaults: defaultResetWindowWeights(),
    minCodexDesktopVersion: '',
    minCodexCliVersion: '',
    usageRetentionDays: 31,
    opsEventRetentionDays: 30,
    auditRetentionDays: 90,
    requestTuning: { ...requestTuningFallbacks, smartScheduling: defaultSmartScheduling() },
  })

  const runtimeUnits = useRuntimeUnits(form)
  const unitFields = runtimeUnits.fields

  const savedDraft = shallowRef('')
  const savedLocation = shallowRef(defaultRequestLocation())
  const draftSnapshot = () => JSON.stringify({
    form,
    mappings: mappings.value,
    units: runtimeUnits.snapshot(),
  })
  const hasChanges = computed(() => savedDraft.value !== '' && savedDraft.value !== draftSnapshot())

  function numericModel(key: 'refreshMarginSeconds' | 'refreshConcurrency' | 'maxConcurrentPerAccount' | 'requestIntervalMs' | 'openaiGuardianReservedConcurrency' | 'sessionAffinityTtlHours') {
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
  const openaiGuardianReservedConcurrencyValue = numericModel('openaiGuardianReservedConcurrency')
  const requestIntervalMsValue = numericModel('requestIntervalMs')
  const responsesMaxDecompressedBodyBytesValue = computed({
    get: () => String(form.responsesMaxDecompressedBodyBytes),
    set: (value: string) => {
      const parsed = Number(value)
      if (Number.isFinite(parsed))
        form.responsesMaxDecompressedBodyBytes = parsed
    },
  })
  const sessionAffinityTtlHoursValue = numericModel('sessionAffinityTtlHours')
  const minCodexDesktopVersionError = computed(() => versionError(form.minCodexDesktopVersion))
  const minCodexCliVersionError = computed(() => versionError(form.minCodexCliVersion))

  function versionError(value: string): string {
    const normalized = value.trim()
    return normalized && !isSemver(normalized) ? t('settings.form.enterStandardSemverForExample') : ''
  }

  function applySettings(data: Awaited<ReturnType<typeof getSettings>>) {
    configRevision.value = data.configRevision
    form.codexPrivacyPolicy = cloneDeep(data.codexPrivacyPolicy ?? { enabled: false, onError: 'skip_rule', rules: [] })
    form.providerRequestProfiles = cloneDeep(data.providerRequestProfiles ?? {})
    savedProfiles.value = cloneDeep(form.providerRequestProfiles)
    form.responsesMaxDecompressedBodyBytes
      = data.responsesMaxDecompressedBodyBytes ?? 64 * 1024 * 1024
    form.accountAffinity = data.accountAffinity ?? 'strict'
    form.openaiRequestSettings = cloneDeep(data.openaiRequestSettings)
    form.openaiWebsocketPool = cloneDeep(data.openaiWebsocketPool)
    form.refreshMarginSeconds = data.refreshMarginSeconds
    form.refreshConcurrency = data.refreshConcurrency
    form.maxConcurrentPerAccount = data.maxConcurrentPerAccount
    form.openaiGuardianReservedConcurrency = data.openaiGuardianReservedConcurrency ?? 0
    form.accountWarmupEnabled = data.accountWarmupEnabled ?? false
    form.accountWarmupCron = data.accountWarmupCron ? data.accountWarmupCron.split(';') : []
    form.accountWarmupModel = data.accountWarmupModel ?? ''
    form.codexQuotaOverdraftEnabled = data.codexQuotaOverdraftEnabled ?? false
    form.codexQuotaOverdraftProbeModel = data.codexQuotaOverdraftProbeModel?.trim() || 'gpt-6-astra'
    form.requestIntervalMs = data.requestIntervalMs
    form.sessionAffinityTtlHours = data.sessionAffinityTtlHours ?? 24
    form.rotationStrategy = data.rotationStrategy
    form.quotaResetWindowWeights = { ...data.quotaResetWindowWeights }
    form.quotaResetWindowWeightsDefaults = { ...data.quotaResetWindowWeightsDefaults }
    form.minCodexDesktopVersion = data.minCodexDesktopVersion ?? ''
    form.minCodexCliVersion = data.minCodexCliVersion ?? ''
    form.usageRetentionDays = data.usageRetentionDays
    form.opsEventRetentionDays = data.opsEventRetentionDays
    form.auditRetentionDays = data.auditRetentionDays
    const tuning = data.requestTuning ?? {}
    form.accountErrorProtection = { ...defaultProtection(), ...tuning.accountErrorProtection, probeModel: tuning.accountErrorProtection?.probeModel ?? '', rules: cloneProtectionRules(tuning.accountErrorProtection?.rules ?? []) }
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
    form.requestTuning.smartScheduling = { ...form.requestTuning.smartScheduling, resetWindowWeights: { ...defaultSmartScheduling().resetWindowWeights, ...form.requestTuning.smartScheduling.resetWindowWeights } }
    form.requestTuning.openaiRequestLocation = {
      ...(tuning.openaiRequestLocation ?? defaultRequestLocation()),
    }
    savedLocation.value = { ...form.requestTuning.openaiRequestLocation }
    runtimeUnits.reset()
    mappings.value = Object.entries(data.modelMappings || {}).map(([requestedModel, upstreamModel]) => ({
      requestedModel,
      upstreamModel: String(upstreamModel),
    }))
    savedDraft.value = draftSnapshot()
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
        throw new Error(t('settings.form.fillInModelMappingCompletely'))
      if (entries[requested])
        throw new Error(t('settings.form.duplicateClientModel', { requested }))
      entries[requested] = upstream
    }
    return entries
  }

  async function saveSettings() {
    const pool = form.openaiWebsocketPool
    if (![pool.maxAgeMs, pool.fastPathBudgetMs, pool.idleTimeoutMs, pool.streamIdleTimeoutMs].every(Number.isSafeInteger)
      || pool.maxAgeMs <= 0 || pool.idleTimeoutMs <= 0 || pool.streamIdleTimeoutMs < 0
      || pool.fastPathBudgetMs < 1 || pool.fastPathBudgetMs > 15000) {
      toast.warning(t('settings.websocketPool.invalidSettings'))
      return
    }
    if (form.openaiRequestSettings.requestCountry.mode === 'custom' && !/^[A-Z]{2}$/.test(form.openaiRequestSettings.requestCountry.country)) {
      toast.warning(t('settings.location.invalidCountryCode'))
      return
    }
    if (saving.value || loading.value)
      return
    if (form.accountAffinity !== 'strict' && form.accountAffinity !== 'relaxed' && form.accountAffinity !== 'preferred') {
      toast.warning('请选择有效的账号亲和模式')
      return
    }
    const ttlHours = form.sessionAffinityTtlHours
    if (ttlHours === null || !Number.isInteger(ttlHours) || ttlHours < 1 || ttlHours > 720) {
      toast.warning('会话账号绑定时长须为 1 至 720 小时的整数')
      return
    }
    const { refreshMarginSeconds, refreshConcurrency, maxConcurrentPerAccount, requestIntervalMs, rotationStrategy } = form
    if (refreshMarginSeconds === null || refreshConcurrency === null || maxConcurrentPerAccount === null || requestIntervalMs === null || !rotationStrategy) {
      toast.warning('请完整填写运行参数和调度策略')
      return
    }
    const openaiGuardianReservedConcurrencyValue = form.openaiGuardianReservedConcurrency
    if (openaiGuardianReservedConcurrencyValue === null
      || !Number.isSafeInteger(openaiGuardianReservedConcurrencyValue)
      || openaiGuardianReservedConcurrencyValue < 0) {
      toast.warning('Guardian 独立并发须为非负整数，0 表示共用普通并发')
      return
    }
    const openaiGuardianReservedConcurrency = openaiGuardianReservedConcurrencyValue
    const accountWarmupModel = form.accountWarmupModel.trim()
    if (!validWarmupExpressions(form.accountWarmupCron, form.accountWarmupEnabled)
      || warmupScheduleBytes(form.accountWarmupCron) > 255
      || (form.accountWarmupEnabled && !accountWarmupModel)) {
      toast.warning('请填写有效的预热 Cron 规则和模型，规则总长度不超过 255 字节')
      return
    }
    const codexQuotaOverdraftProbeModel = form.codexQuotaOverdraftProbeModel.trim()
    const hasControlCharacter = [...codexQuotaOverdraftProbeModel]
      .some(character => character.charCodeAt(0) < 32 || character.charCodeAt(0) === 127)
    if ((form.codexQuotaOverdraftEnabled && !codexQuotaOverdraftProbeModel)
      || codexQuotaOverdraftProbeModel.length > 128
      || hasControlCharacter) {
      toast.warning('透支探测模型须为 1–128 个字符，且不能包含控制字符')
      return
    }
    const effectiveCodexQuotaOverdraftProbeModel = codexQuotaOverdraftProbeModel || 'gpt-6-astra'
    if (minCodexDesktopVersionError.value || minCodexCliVersionError.value) {
      toast.warning('请修正客户端最低版本格式')
      return
    }
    const protectionError = protectionSettingsError(form.accountErrorProtection)
    if (protectionError) {
      toast.warning(protectionError)
      return
    }
    if (runtimeUnits.invalid.value) {
      toast.warning('请填写有效数值，换算后须为整数的字节数或毫秒数')
      return
    }
    const tuning = form.requestTuning
    if (!Number.isInteger(tuning.maxRetryAfterSeconds) || tuning.maxRetryAfterSeconds < 0 || tuning.maxRetryAfterSeconds > 4_294_967_295) {
      toast.warning('上游建议等待上限须为 0–4294967295 的整数秒')
      return
    }
    if (!Object.values(form.quotaResetWindowWeights).every(value => Number.isFinite(value) && value >= 0 && value <= 10 && Math.round(value * 10) / 10 === value)) {
      toast.warning('额度调度系数须为 0–10，最多一位小数')
      return
    }
    const smartWeights = [
      tuning.smartScheduling.loadWeight,
      tuning.smartScheduling.quotaWeight,
      tuning.smartScheduling.healthWeight,
      tuning.smartScheduling.latencyWeight,
      tuning.smartScheduling.resetWeight,
      tuning.smartScheduling.queueWeight,
    ]
    if (![...smartWeights, ...Object.values(tuning.smartScheduling.resetWindowWeights)].every(value => Number.isFinite(value) && value >= 0 && value <= 10 && Math.round(value * 10) / 10 === value)
      || !smartWeights.some(value => value > 0)) {
      toast.warning('智能调度权重须为 0–10，最多一位小数，且不能全部为 0')
      return
    }
    if (!Number.isInteger(form.responsesMaxDecompressedBodyBytes)
      || form.responsesMaxDecompressedBodyBytes < 1
      || form.responsesMaxDecompressedBodyBytes > 256 * 1024 * 1024) {
      toast.warning('请求解压上限须为 1–268435456 的整数字节数')
      return
    }
    if (!Number.isInteger(tuning.maxWaitingPerKey) || tuning.maxWaitingPerKey < 0 || tuning.maxWaitingPerKey > 1024
      || !Number.isInteger(tuning.keyQueueTimeoutSeconds) || tuning.keyQueueTimeoutSeconds < 1 || tuning.keyQueueTimeoutSeconds > 120) {
      toast.warning('Key 等待人数须为 0–1024 的整数，等待秒数须为 1–120 的整数')
      return
    }
    if (!Number.isInteger(tuning.maxWaitingPerAccount) || tuning.maxWaitingPerAccount < 1 || tuning.maxWaitingPerAccount > 1000
      || !Number.isInteger(tuning.accountQueueTimeoutSeconds) || tuning.accountQueueTimeoutSeconds < 1 || tuning.accountQueueTimeoutSeconds > 120) {
      toast.warning('账号忙时等待人数须为 1–1000 的整数，等待秒数须为 1–120 的整数')
      return
    }
    // Disabled overlays retain the saved value, not an unfinished editor draft.
    const requestLocation = tuning.openaiLocationOverrideEnabled
      ? normalizeRequestLocation(tuning.openaiRequestLocation ?? defaultRequestLocation())
      : savedLocation.value
    const locationError = requestLocationError(requestLocation)
    if (locationError) {
      toast.warning(locationError)
      return
    }
    await saveAction.run(async () => {
      const result = await updateSettings({
        configRevision: configRevision.value,
        codexPrivacyPolicy: cloneDeep(form.codexPrivacyPolicy),
        providerRequestProfiles: requestProfileChanges(savedProfiles.value, form.providerRequestProfiles),
        responsesMaxDecompressedBodyBytes: form.responsesMaxDecompressedBodyBytes,
        modelMappings: mappingPayload(),
        openaiRequestSettings: cloneDeep(form.openaiRequestSettings),
        openaiWebsocketPool: cloneDeep(form.openaiWebsocketPool),
        refreshMarginSeconds,
        refreshConcurrency,
        maxConcurrentPerAccount,
        openaiGuardianReservedConcurrency,
        accountWarmupEnabled: form.accountWarmupEnabled,
        accountWarmupCron: form.accountWarmupCron.join(';'),
        accountWarmupModel: accountWarmupModel || null,
        codexQuotaOverdraftEnabled: form.codexQuotaOverdraftEnabled,
        codexQuotaOverdraftProbeModel: effectiveCodexQuotaOverdraftProbeModel,
        requestIntervalMs,
        sessionAffinityTtlHours: ttlHours,
        accountAffinity: form.accountAffinity,
        rotationStrategy,
        quotaResetWindowWeights: { ...form.quotaResetWindowWeights },
        minCodexDesktopVersion: form.minCodexDesktopVersion.trim() || null,
        minCodexCliVersion: form.minCodexCliVersion.trim() || null,
        usageRetentionDays: form.usageRetentionDays,
        opsEventRetentionDays: form.opsEventRetentionDays,
        auditRetentionDays: form.auditRetentionDays,
        requestTuning: {
          ...form.requestTuning,
          accountErrorProtection: { ...form.accountErrorProtection, rules: cloneProtectionRules(form.accountErrorProtection.rules), threshold: form.accountErrorProtection.threshold!, windowSeconds: form.accountErrorProtection.windowSeconds!, durationSeconds: form.accountErrorProtection.durationSeconds!, probeModel: trimProtectionText(form.accountErrorProtection.probeModel) || null },
          smartScheduling: { ...form.requestTuning.smartScheduling, resetWindowWeights: { ...form.requestTuning.smartScheduling.resetWindowWeights } },
          openaiRequestLocation: { ...requestLocation },
        },
      })
      applySettings(result)
      toast.success('设置已保存')
    }, {
      onError: (cause) => {
        // 明确拒绝的请求保留草稿，执行结果未知时仍回读设置以同步已提交的版本
        if (cause instanceof ApiError && cause.status !== 409
          && (cause.kind !== 'api' || cause.status < 400 || cause.status >= 500)) {
          void loadSettings(true)
        }
      },
    })
  }

  return {
    hasChanges,
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
    openaiGuardianReservedConcurrencyValue,
    unitFields,
    requestIntervalMsValue,
    responsesMaxDecompressedBodyBytesValue,
    sessionAffinityTtlHoursValue,
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
