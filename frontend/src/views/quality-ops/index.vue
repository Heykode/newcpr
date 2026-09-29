<script setup lang="ts">
import type { QualityRule, QualityRuleConfig, QualityRuleTemplate, QualityRun } from '@/api/modules/quality-ops'
import { ClipboardCheck, Copy, Eye, Pause, Pencil, Play, Plus, RefreshCw, Save, Trash2 } from '@lucide/vue'
import { computed, onBeforeUnmount, onMounted, ref, toRaw, watch } from 'vue'
import { useRoute } from 'vue-router'
import { getAccountGroups } from '@/api/modules/account-groups'
import { getAccounts } from '@/api/modules/accounts'
import { deleteQualityRule, deleteQualityTemplate, getQualityDetail, getQualityRules, getQualityRuns, getQualityTemplates, runQualityRule, saveQualityRule, saveQualityTemplate } from '@/api/modules/quality-ops'
import AccountTemplatePicker from '@/components/account-templates/AccountTemplatePicker.vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import FormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseNumberInput from '@/components/base/BaseNumberInput.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import BaseTextarea from '@/components/base/BaseTextarea.vue'
import { formatDateTime } from '@/utils/date'
import { failureActionOptions, usesFailureThreshold } from './failure-actions'
import { CANDY_PROMPT, CANDY_REFERENCE_ANSWER, DEFAULT_JUDGE_PROMPT } from './presets'
import QualityBulkEditor from './QualityBulkEditor.vue'
import QualityCatalogPicker from './QualityCatalogPicker.vue'
import QualityDrawer from './QualityDrawer.vue'
import QualitySchedule from './QualitySchedule.vue'
import QualityTemplateCatalog from './QualityTemplateCatalog.vue'
import { DEFAULT_QUALITY_INTERVAL_SECONDS, qualityScheduleSummary } from './schedule'

const route = useRoute()
const activeTab = ref<'rules' | 'templates'>(route.query.tab === 'templates' ? 'templates' : 'rules')
const templates = ref<QualityRuleTemplate[]>([])
const templatesLoading = ref(false)
const templatesError = ref('')
const templateMode = ref(false)
const editingTemplate = ref<QualityRuleTemplate | null>(null)
const templateName = ref('')
const deleteTemplateTarget = ref<QualityRuleTemplate | null>(null)
const deleteTemplateOpen = ref(false)
const rules = ref<QualityRule[]>([])
const runs = ref<QualityRun[]>([])
const selectedId = ref('')
const selected = computed(() => rules.value.find(rule => rule.id === selectedId.value))
const filter = ref('')
const loading = ref(false)
const historyLoading = ref(false)
const busy = ref(false)
const error = ref('')
const historyError = ref('')
const editorError = ref('')
const editorOpen = ref(false)
const bulkOpen = ref(false)
const batchSelection = ref<string[]>([])
const editing = ref<QualityRule | null>(null)
const selectedAccounts = ref<string[]>([])
const actions: Record<string, string> = {
  template_applied: '已应用账号模板',
  template_applied_probe_paused: '已应用账号模板并暂停状态探针',
  template_unavailable: '模板已删除或版本变化，请重新选择',
  template_blocked_references: '模板分组或代理不可用，未修改账号',
  template_blocked_settings: '模板代理或 IPv6 等设置冲突，未修改账号',
  template_blocked_account: '账号当前不可用或已被独立暂停，未应用模板',
  scheduling_paused: '已暂停账号调度',
  groups_removed: '已移出指定分组',
  restored: '已恢复质量检测前的状态',
  restore_blocked: '恢复受阻：账号状态或分组已变化，请检查',
  ownership_released: '已保留人工设置，不自动恢复',
  already_applied: '保持此前的质量处置',
  no_change: '账号原已暂停或不在指定分组，未改动',
  identity_changed: '检测期间账号身份已变化，未执行处置',
  excel_enabled: '已开启 Excel 模式',
  excel_enabled_probe_paused: '已开启 Excel 模式，探针规则已暂停',
  probe_paused_excel: '账号已开启 Excel 模式，探针规则已暂停',
  excel_already_enabled: '账号已开启 Excel 模式',
  excel_blocked_configuration_changed: '检测期间配置已变化，未开启 Excel',
  excel_blocked_403: 'Excel 曾因 HTTP 403 关闭，未自动重开',
  excel_blocked_model: '检测模型未配置为 Excel 模型，未改动账号',
  excel_blocked_account: '账号当前不可用，未开启 Excel',
  excel_threshold_pending: '连续异常尚未达到阈值，未执行处置',
  excel_streak_reset: '本轮正常，连续异常计数已清零',
}
const deleteTarget = ref<QualityRule | null>(null)
const deleteOpen = ref(false)
const detailOpen = ref(false)
const detail = ref<QualityRun | null>(null)
const detailError = ref('')
const names = ref<Record<string, string>>({})
const efforts = ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'].map(value => ({ value, label: value }))
const labels: Record<string, string> = {
  correct: '通过',
  incorrect: '答案不符',
  unknown: '不确定',
  request_error: '请求错误',
  running: '检测中',
  interrupted: '执行中断',
  cancelled: '已取消',
}
const visibleRules = computed(() => rules.value.filter(rule => `${names.value[rule.config.accountId] ?? ''} ${rule.config.accountId} ${rule.config.model}`.toLowerCase().includes(filter.value.toLowerCase())))
const allVisibleSelected = computed({
  get: () => visibleRules.value.length > 0 && visibleRules.value.every(rule => batchSelection.value.includes(rule.id)),
  set: (checked: boolean) => {
    const visible = new Set(visibleRules.value.map(rule => rule.id))
    batchSelection.value = checked
      ? [...new Set([...batchSelection.value, ...visible])]
      : batchSelection.value.filter(id => !visible.has(id))
  },
})
const someVisibleSelected = computed(() => visibleRules.value.some(rule => batchSelection.value.includes(rule.id)))
function selectBatchRule(id: string, checked: boolean) {
  batchSelection.value = checked ? [...new Set([...batchSelection.value, id])] : batchSelection.value.filter(value => value !== id)
}
const counts = computed(() => ({
  enabled: rules.value.filter(rule => rule.config.enabled).length,
  running: rules.value.filter(rule => rule.running).length,
  incorrect: rules.value.filter(rule => rule.lastStatus === 'incorrect').length,
  errors: rules.value.filter(rule => rule.lastStatus === 'request_error').length,
}))
function defaults(): QualityRuleConfig {
  return {
    detectionMode: 'answer',
    accountId: '',
    model: '',
    enabled: true,
    intervalSeconds: DEFAULT_QUALITY_INTERVAL_SECONDS,
    cron: '',
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC',
    repetitions: 1,
    prompt: CANDY_PROMPT,
    referenceAnswer: CANDY_REFERENCE_ANSWER,
    reasoningEffort: null,
    judgeGroupId: '',
    judgeModel: '',
    judgePrompt: DEFAULT_JUDGE_PROMPT,
    failureAction: 'none',
    failureTemplate: null,
    failureGroupIds: [],
    autoRestore: false,
    excelFailureThreshold: 1,
  }
}
const draft = ref(defaults())
const selectedTemplate = computed({
  get: () => draft.value.failureTemplate ?? null,
  set: (value) => { draft.value.failureTemplate = value },
})
const isProbe = computed(() => draft.value.detectionMode === 'state_probe')
watch(() => draft.value.failureAction, (action) => {
  if (usesFailureThreshold(action))
    draft.value.autoRestore = false
})
function verdictLabel(status: string | null, mode?: QualityRuleConfig['detectionMode']) {
  if (mode === 'state_probe') {
    const probeLabels: Record<string, string> = { correct: '智商正常', incorrect: '智商异常', unknown: '无法判断', request_error: '无法判断' }
    if (status && probeLabels[status])
      return probeLabels[status]
  }
  return labels[status ?? ''] ?? status ?? '等待首次检测'
}
function modeLabel(mode?: QualityRuleConfig['detectionMode']) {
  return mode === 'state_probe' ? '状态探针' : '题目检测'
}
const effort = computed({
  get: () => draft.value.reasoningEffort ?? '',
  set: (value: string) => {
    draft.value.reasoningEffort = value || null
  },
})
let alive = true
let timer: ReturnType<typeof setTimeout> | undefined
let listController: AbortController | undefined
let historyController: AbortController | undefined
let detailController: AbortController | undefined
let catalogController: AbortController | undefined
let templatesController: AbortController | undefined
let listVersion = 0

function message(cause: unknown) {
  return cause instanceof Error ? cause.message : '操作失败，请重试'
}
function statusColor(status: string | null) {
  return status === 'correct' ? 'text-cp-success' : status === 'incorrect' || status === 'request_error' ? 'text-cp-error' : 'text-cp-text-secondary'
}
function ruleStatusColor(rule: QualityRule) {
  if (rule.running || rule.pending)
    return 'text-cp-primary'
  return statusColor(rule.config.enabled ? rule.lastStatus : null)
}
function accountName(id: string) {
  return names.value[id] || id
}

async function load() {
  const version = ++listVersion
  listController?.abort()
  const controller = new AbortController()
  listController = controller
  loading.value = true
  try {
    const result = await getQualityRules({ signal: controller.signal, silent: true })
    if (!alive || controller.signal.aborted || version !== listVersion)
      return
    rules.value = result
    batchSelection.value = batchSelection.value.filter(id => result.some(rule => rule.id === id))
    error.value = ''
    if (!result.some(rule => rule.id === selectedId.value))
      selectedId.value = linkedRuleId() || result[0]?.id || ''
  }
  catch (cause) {
    if (alive && !controller.signal.aborted)
      error.value = message(cause)
  }
  finally {
    if (alive && version === listVersion)
      loading.value = false
  }
}
function linkedRuleId() {
  return rules.value.find(rule => rule.id === route.query.ruleId || rule.config.accountId === route.query.accountId)?.id
}
watch(() => [route.query.accountId, route.query.ruleId, route.query.tab], () => {
  activeTab.value = route.query.tab === 'templates' ? 'templates' : 'rules'
  const id = linkedRuleId()
  if (id) {
    selectedId.value = id
    filter.value = ''
  }
})
async function loadTemplates() {
  templatesController?.abort()
  const controller = new AbortController()
  templatesController = controller
  templatesLoading.value = true
  try {
    const result = await getQualityTemplates({ signal: controller.signal, silent: true })
    if (alive && !controller.signal.aborted) {
      templates.value = result
      templatesError.value = ''
    }
  }
  catch (cause) {
    if (alive && !controller.signal.aborted)
      templatesError.value = message(cause)
  }
  finally {
    if (alive && templatesController === controller)
      templatesLoading.value = false
  }
}
async function history() {
  historyController?.abort()
  const id = selectedId.value
  if (!id) {
    runs.value = []
    return
  }
  const controller = new AbortController()
  historyController = controller
  historyLoading.value = true
  try {
    const result = await getQualityRuns(id, { signal: controller.signal, silent: true })
    if (alive && !controller.signal.aborted && selectedId.value === id) {
      runs.value = result
      historyError.value = ''
    }
  }
  catch (cause) {
    if (alive && !controller.signal.aborted)
      historyError.value = message(cause)
  }
  finally {
    if (alive && !controller.signal.aborted)
      historyLoading.value = false
  }
}
watch(selectedId, () => {
  runs.value = []
  historyError.value = ''
  void history()
})
async function poll() {
  if (!document.hidden) {
    await load()
    await history()
  }
  if (alive)
    timer = setTimeout(poll, 10000)
}
async function refresh() {
  if (activeTab.value === 'templates') {
    await loadTemplates()
    return
  }
  await load()
  await history()
}
async function accountPage(page: number, search: string, signal: AbortSignal) {
  const result = await getAccounts({ page, pageSize: 50, search: search || undefined }, { signal, silent: true })
  if (!signal.aborted && alive) {
    for (const account of result.items)
      names.value[account.id] = account.name
  }
  return {
    items: result.items.map(account => ({ value: account.id, label: account.name, description: account.email && account.email !== account.name ? account.email : undefined })),
    total: result.page.total,
    totalPages: result.page.totalPages,
  }
}
async function groupPage(page: number, search: string, signal: AbortSignal) {
  const result = await getAccountGroups({ page, pageSize: 50, search: search || undefined, enabled: true }, { signal, silent: true })
  return {
    items: result.items.map(group => ({ value: group.id, label: group.name, description: `${group.memberCount} 个账号` })),
    total: result.page.total,
    totalPages: result.page.totalPages,
  }
}
async function actionGroupPage(page: number, search: string, signal: AbortSignal) {
  const result = await getAccountGroups({ page, pageSize: 50, search: search || undefined }, { signal, silent: true })
  return {
    items: result.items.map(group => ({ value: group.id, label: group.name, description: `${group.memberCount} 个账号${group.enabled ? '' : ' · 分组已停用'}` })),
    total: result.page.total,
    totalPages: result.page.totalPages,
  }
}
async function loadAccountNames() {
  catalogController?.abort()
  const controller = new AbortController()
  catalogController = controller
  try {
    await accountPage(1, '', controller.signal)
  }
  catch {
    // Rules remain usable with account IDs; the picker owns retryable load errors.
  }
}
function edit(rule: QualityRule | null) {
  templateMode.value = false
  editing.value = rule ? { ...rule, config: { ...rule.config } } : null
  draft.value = rule ? { ...defaults(), ...rule.config, intervalSeconds: rule.config.intervalSeconds ?? null, failureGroupIds: [...(rule.config.failureGroupIds ?? [])] } : defaults()
  selectedAccounts.value = rule ? [rule.config.accountId] : []
  editorError.value = ''
  editorOpen.value = true
}
function editTemplate(template: QualityRuleTemplate | null, rule?: QualityRule) {
  templateMode.value = true
  editing.value = null
  editingTemplate.value = template ? { ...template } : null
  templateName.value = template?.name ?? ''
  const config = template?.config ?? rule?.config
  draft.value = config ? { ...defaults(), ...structuredClone(toRaw(config)), intervalSeconds: config.intervalSeconds ?? null, accountId: '' } : defaults()
  selectedAccounts.value = []
  editorError.value = ''
  editorOpen.value = true
}
async function save() {
  if (busy.value)
    return
  busy.value = true
  const config = { ...draft.value, failureGroupIds: [...draft.value.failureGroupIds] }
  if (config.detectionMode === 'state_probe') {
    config.repetitions = 1
    config.reasoningEffort = null
  }
  if (usesFailureThreshold(config.failureAction))
    config.autoRestore = false
  if (config.failureAction !== 'apply_account_template')
    delete config.failureTemplate
  const accountIds = editing.value ? [config.accountId] : [...selectedAccounts.value]
  const failures: string[] = []
  try {
    if (templateMode.value) {
      const { accountId: _accountId, ...templateConfig } = config
      await saveQualityTemplate({ id: editingTemplate.value?.id ?? null, revision: editingTemplate.value?.revision ?? null, name: templateName.value.trim(), config: templateConfig })
      if (alive) {
        editorOpen.value = false
        activeTab.value = 'templates'
        await loadTemplates()
      }
      return
    }
    for (const accountId of accountIds) {
      if (!alive)
        return
      try {
        const result = await saveQualityRule({ id: editing.value?.id ?? null, revision: editing.value?.revision ?? null, config: { ...config, accountId } })
        if (!alive)
          return
        selectedId.value = result.id
        selectedAccounts.value = selectedAccounts.value.filter(id => id !== accountId)
      }
      catch (cause) {
        failures.push(`${accountName(accountId)}：${message(cause)}`)
      }
    }
    editorOpen.value = failures.length > 0
    editorError.value = failures.length ? `${failures.length} 项未保存；成功项已保留。${failures.slice(0, 5).join('；')}` : ''
    await load()
    await history()
  }
  catch (cause) {
    if (alive)
      editorError.value = message(cause)
  }
  finally { busy.value = false }
}
async function confirmDeleteTemplate() {
  if (busy.value || !deleteTemplateTarget.value)
    return
  busy.value = true
  try {
    const { id, revision } = deleteTemplateTarget.value
    await deleteQualityTemplate({ id, revision })
    if (alive) {
      deleteTemplateOpen.value = false
      deleteTemplateTarget.value = null
      await loadTemplates()
    }
  }
  catch (cause) {
    if (alive)
      templatesError.value = message(cause)
  }
  finally { busy.value = false }
}
async function mutate(action: () => Promise<unknown>) {
  if (busy.value)
    return
  busy.value = true
  try {
    await action()
    if (!alive)
      return
    deleteOpen.value = false
    deleteTarget.value = null
    await load()
    await history()
  }
  catch (cause) {
    if (alive)
      error.value = message(cause)
  }
  finally { busy.value = false }
}
function toggle(rule: QualityRule) {
  void mutate(() => saveQualityRule({ id: rule.id, revision: rule.revision, config: { ...rule.config, enabled: !rule.config.enabled } }))
}
function remove(rule: QualityRule) {
  deleteTarget.value = { ...rule, config: { ...rule.config } }
  deleteOpen.value = true
}
function confirmDelete() {
  const rule = deleteTarget.value
  if (rule)
    void mutate(() => deleteQualityRule({ id: rule.id, revision: rule.revision }))
}
async function show(run: QualityRun) {
  detailController?.abort()
  const controller = new AbortController()
  detailController = controller
  detail.value = null
  detailError.value = ''
  detailOpen.value = true
  try {
    const result = await getQualityDetail(run.id, { signal: controller.signal, silent: true })
    if (alive && !controller.signal.aborted)
      detail.value = result
  }
  catch (cause) {
    if (alive && !controller.signal.aborted)
      detailError.value = message(cause)
  }
}
watch(detailOpen, (open) => {
  if (!open) {
    detailController?.abort()
    detail.value = null
  }
})
onMounted(() => {
  void loadAccountNames()
  void loadTemplates()
  void poll()
})
onBeforeUnmount(() => {
  alive = false
  clearTimeout(timer)
  for (const controller of [listController, historyController, detailController, catalogController, templatesController])
    controller?.abort()
})
</script>

<template>
  <div class="flex min-w-0 flex-col gap-5">
    <BasePageHeader title="质量运维" class="quality-header">
      <template #actions>
        <BaseIconButton label="刷新" :disabled="loading || historyLoading" @click="refresh">
          <RefreshCw class="size-4" />
        </BaseIconButton>
        <BaseButton v-if="activeTab === 'rules'" variant="primary" :disabled="busy" @click="edit(null)">
          <Plus class="size-4" />新建规则
        </BaseButton>
      </template>
    </BasePageHeader>
    <p v-if="error" role="alert" class="text-cp-error">
      {{ error }}
    </p>
    <nav class="flex gap-5 border-b border-cp-border" aria-label="质量运维视图">
      <button v-for="tab in [{ value: 'rules' as const, label: '账号监测' }, { value: 'templates' as const, label: '规则模板' }]" :key="tab.value" type="button" class="border-b-2 px-1 py-3 text-cp-sm font-semibold" :class="activeTab === tab.value ? 'border-cp-primary text-cp-primary' : 'border-transparent text-cp-text-secondary'" :aria-pressed="activeTab === tab.value" @click="activeTab = tab.value">
        {{ tab.label }}
      </button>
    </nav>
    <QualityTemplateCatalog v-if="activeTab === 'templates'" :templates="templates" :loading="templatesLoading" :busy="busy" :error="templatesError" @create="editTemplate(null)" @edit="editTemplate($event)" @remove="deleteTemplateTarget = $event; deleteTemplateOpen = true" @refresh="loadTemplates" />
    <div v-if="activeTab === 'rules'" class="grid grid-cols-2 border-y border-cp-border sm:grid-cols-4">
      <div class="border-b border-cp-border px-4 py-3 sm:border-b-0 sm:border-r">
        <div class="flex items-center gap-3">
          <span class="text-cp-sm text-cp-text-secondary">启用规则</span>
          <strong class="text-xl tabular-nums">{{ counts.enabled }}</strong>
        </div>
      </div>
      <div class="border-b border-cp-border px-4 py-3 sm:border-b-0 sm:border-r">
        <div class="flex items-center gap-3">
          <span class="text-cp-sm text-cp-text-secondary">检测中</span>
          <strong class="text-xl tabular-nums">{{ counts.running }}</strong>
        </div>
      </div>
      <div class="px-4 py-3 sm:border-r sm:border-cp-border">
        <div class="flex items-center gap-3">
          <span class="text-cp-sm text-cp-text-secondary">最近检测异常</span>
          <strong class="text-xl tabular-nums text-cp-warning">{{ counts.incorrect }}</strong>
        </div>
      </div>
      <div class="px-4 py-3">
        <div class="flex items-center gap-3">
          <span class="text-cp-sm text-cp-text-secondary">最近请求错误</span>
          <strong class="text-xl tabular-nums text-cp-error">{{ counts.errors }}</strong>
        </div>
      </div>
    </div>
    <div v-if="activeTab === 'rules'" class="grid min-w-0 gap-5 xl:grid-cols-[minmax(260px,340px)_minmax(0,1fr)]">
      <section class="min-w-0">
        <div class="mb-3 flex items-center justify-between gap-3">
          <h2 class="text-base font-semibold">
            检测规则
          </h2>
          <span class="text-cp-sm tabular-nums text-cp-text-secondary">{{ rules.length }}</span>
        </div>
        <BaseInput v-model="filter" aria-label="筛选规则" placeholder="搜索账号或模型" />
        <div class="mt-3 flex flex-wrap items-center justify-between gap-3 text-cp-sm">
          <BaseCheckbox v-model="allVisibleSelected" label="全选搜索结果" :indeterminate="someVisibleSelected && !allVisibleSelected" :disabled="busy || !visibleRules.length" show-label />
          <BaseButton :disabled="busy || !batchSelection.length" @click="bulkOpen = true">
            <Pencil class="size-4" />批量编辑（{{ batchSelection.length }}）
          </BaseButton>
        </div>
        <div class="mt-3 max-h-[65vh] space-y-1 overflow-y-auto pr-1">
          <div v-for="rule in visibleRules" :key="rule.id" class="quality-rule rounded-lg border py-3 transition-colors" :class="selectedId === rule.id ? 'quality-rule-selected border-cp-border bg-cp-bg-container' : 'border-transparent hover:bg-cp-bg-container'">
            <button type="button" class="grid w-full min-w-0 gap-1 px-3 text-left text-cp-text outline-none focus-visible:ring-2 focus-visible:ring-cp-control-outline" :aria-pressed="selectedId === rule.id" @click="selectedId = rule.id">
              <span class="truncate font-semibold" :title="accountName(rule.config.accountId)">{{ accountName(rule.config.accountId) }}</span>
              <span class="truncate font-mono text-cp-sm text-cp-text-secondary" :title="rule.config.model">{{ rule.config.model }}</span>
              <span class="text-xs text-cp-text-secondary">{{ modeLabel(rule.config.detectionMode) }}</span>
              <span v-if="rule.sourceTemplate" class="truncate text-xs text-cp-text-secondary" :title="`版本 ${rule.sourceTemplate.revision}`">来源模板：{{ rule.sourceTemplate.name }}</span>
              <span v-if="usesFailureThreshold(rule.config.failureAction)" class="text-xs tabular-nums text-cp-text-secondary">连续异常 {{ rule.excelFailureStreak ?? 0 }}/{{ rule.config.excelFailureThreshold ?? 1 }} 轮</span>
              <span v-if="rule.config.failureAction === 'apply_account_template'" class="truncate text-xs text-cp-text-secondary">模板：{{ rule.config.failureTemplate?.config.name ?? '未选择' }}</span>
              <span class="mt-1 flex items-center gap-1.5 text-xs" :class="ruleStatusColor(rule)"><span class="size-1.5 shrink-0 rounded-full bg-current" aria-hidden="true" />{{ rule.running ? '检测中' : rule.pending ? '已排队' : !rule.config.enabled ? '已暂停' : verdictLabel(rule.lastStatus, rule.config.detectionMode) }}</span>
              <span class="text-xs text-cp-text-secondary">下次 {{ rule.config.enabled ? formatDateTime(rule.nextRunAt) : '—' }}</span>
            </button>
            <div class="mt-2 flex gap-1 px-2">
              <BaseCheckbox :model-value="batchSelection.includes(rule.id)" :label="`选择规则 ${accountName(rule.config.accountId)}`" :disabled="busy" class="mx-1" @update:model-value="selectBatchRule(rule.id, $event)" />
              <BaseIconButton label="立即检测" :disabled="busy || rule.running || rule.pending" @click="mutate(() => runQualityRule({ id: rule.id, revision: rule.revision }))">
                <Play class="size-4" />
              </BaseIconButton>
              <BaseIconButton :label="rule.config.enabled ? '暂停定时检测' : '启用定时检测'" :disabled="busy" @click="toggle(rule)">
                <Pause v-if="rule.config.enabled" class="size-4" /><ClipboardCheck v-else class="size-4" />
              </BaseIconButton>
              <BaseIconButton label="编辑规则" :disabled="busy" @click="edit(rule)">
                <Pencil class="size-4" />
              </BaseIconButton>
              <BaseIconButton label="另存为规则模板" :disabled="busy" @click="editTemplate(null, rule)">
                <Copy class="size-4" />
              </BaseIconButton>
              <BaseIconButton label="删除规则" :disabled="busy" @click="remove(rule)">
                <Trash2 class="size-4" />
              </BaseIconButton>
            </div>
          </div>
          <p v-if="!visibleRules.length" class="py-10 text-center text-cp-text-secondary">
            {{ loading ? '加载中…' : '暂无规则' }}
          </p>
        </div>
      </section>
      <section class="min-w-0 xl:border-l xl:border-cp-border xl:pl-6">
        <div v-if="selected" class="mb-5 min-w-0 border-b border-cp-border pb-4">
          <div class="flex min-w-0 items-center justify-between gap-3">
            <p class="min-w-0 truncate text-base font-semibold" :title="accountName(selected.config.accountId)">
              {{ accountName(selected.config.accountId) }}
            </p>
            <span class="shrink-0 text-xs" :class="ruleStatusColor(selected)">{{ selected.running ? '检测中' : selected.pending ? '已排队' : selected.config.enabled ? '定时开启' : '已暂停' }}</span>
          </div>
          <p class="mt-1 truncate font-mono text-xs text-cp-text-secondary" :title="selected.config.model">
            {{ selected.config.model }}
          </p>
          <dl class="mt-4 grid min-w-0 grid-cols-2 gap-x-4 gap-y-3 text-xs sm:grid-cols-3">
            <div>
              <dt class="text-cp-text-secondary">
                {{ selected.config.detectionMode === 'state_probe' ? '检测模式' : '每轮次数' }}
              </dt>
              <dd class="mt-1 font-medium tabular-nums">
                {{ selected.config.detectionMode === 'state_probe' ? '状态探针 · 两步检测' : `${selected.config.repetitions} 次` }}
              </dd>
            </div>
            <div class="min-w-0">
              <dt class="text-cp-text-secondary">
                定时计划
              </dt>
              <dd class="mt-1 break-words">
                {{ qualityScheduleSummary(selected.config) }}
              </dd>
            </div>
            <div class="col-span-2 min-w-0 sm:col-span-1">
              <dt class="text-cp-text-secondary">
                下次执行
              </dt>
              <dd class="mt-1 tabular-nums">
                {{ selected.config.enabled ? formatDateTime(selected.nextRunAt) : '—' }}
              </dd>
            </div>
          </dl>
          <p v-if="selected.lastAction" class="mt-3 text-xs text-cp-text-secondary">
            {{ actions[selected.lastAction] || selected.lastAction }}
          </p>
        </div>
        <div class="mb-3 flex items-center justify-between gap-3">
          <h2 class="text-base font-semibold">
            检测记录
          </h2>
          <span class="text-xs tabular-nums text-cp-text-secondary">{{ runs.length }} 条</span>
        </div>
        <p v-if="historyError" role="alert" class="text-cp-error">
          {{ historyError }}
        </p>
        <div class="divide-y divide-cp-border sm:hidden">
          <article v-for="run in runs" :key="run.id" class="py-3 text-cp-sm">
            <div class="flex items-center justify-between gap-2">
              <div class="min-w-0">
                <span class="font-medium" :class="statusColor(run.status)">{{ verdictLabel(run.status, run.detectionMode) }}</span>
                <p class="mt-1 text-xs tabular-nums text-cp-text-secondary">
                  {{ formatDateTime(run.startedAt) }}
                </p>
              </div>
              <BaseIconButton label="查看结果" @click="show(run)">
                <Eye class="size-4" />
              </BaseIconButton>
            </div>
            <dl class="mt-3 grid grid-cols-4 gap-2 text-xs">
              <div>
                <dt class="text-cp-text-secondary">
                  通过
                </dt><dd class="mt-1 font-medium tabular-nums">
                  {{ run.correct }}
                </dd>
              </div>
              <div>
                <dt class="text-cp-text-secondary">
                  不符
                </dt><dd class="mt-1 font-medium tabular-nums">
                  {{ run.incorrect }}
                </dd>
              </div>
              <div>
                <dt class="text-cp-text-secondary">
                  不确定
                </dt><dd class="mt-1 font-medium tabular-nums">
                  {{ run.unknown }}
                </dd>
              </div>
              <div>
                <dt class="text-cp-text-secondary">
                  错误
                </dt><dd class="mt-1 font-medium tabular-nums">
                  {{ run.requestErrors }}
                </dd>
              </div>
            </dl>
          </article>
          <p v-if="!runs.length" class="py-12 text-center text-cp-sm text-cp-text-secondary">
            {{ historyLoading ? '加载中…' : '暂无检测记录' }}
          </p>
        </div>
        <div class="hidden overflow-x-auto sm:block">
          <table class="w-full min-w-[620px] text-left text-cp-sm">
            <thead>
              <tr class="border-b border-cp-border text-cp-text-secondary">
                <th class="px-3 pb-3 pt-1 font-medium">
                  时间
                </th><th class="px-3 pb-3 pt-1 font-medium">
                  结果
                </th><th class="px-3 pb-3 pt-1 font-medium">
                  通过 / 不符
                </th><th class="px-3 pb-3 pt-1 font-medium">
                  不确定 / 错误
                </th><th class="px-3 pb-3 pt-1 font-medium">
                  详情
                </th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="run in runs" :key="run.id" class="border-b border-cp-border last:border-b-0">
                <td class="whitespace-nowrap px-3 py-3">
                  {{ formatDateTime(run.startedAt) }}
                </td>
                <td class="whitespace-nowrap px-3 py-3" :class="statusColor(run.status)">
                  {{ verdictLabel(run.status, run.detectionMode) }}
                </td>
                <td class="px-3 py-3 tabular-nums">
                  {{ run.correct }} / {{ run.incorrect }}
                </td>
                <td class="px-3 py-3 tabular-nums">
                  {{ run.unknown }} / {{ run.requestErrors }}
                </td>
                <td class="px-3 py-3">
                  <BaseIconButton label="查看结果" @click="show(run)">
                    <Eye class="size-4" />
                  </BaseIconButton>
                </td>
              </tr>
              <tr v-if="!runs.length">
                <td colspan="5" class="px-3 py-14 text-center text-cp-text-secondary">
                  {{ historyLoading ? '加载中…' : '暂无检测记录' }}
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>
    </div>
    <QualityDrawer v-model="editorOpen" :title="templateMode ? (editingTemplate ? '编辑规则模板' : '新建规则模板') : editing ? '编辑检测规则' : '新建检测规则'" :busy="busy">
      <form id="quality-rule-form" class="grid min-w-0 gap-4" @submit.prevent="save">
        <p v-if="editorError" role="alert" class="text-cp-error">
          {{ editorError }}
        </p>
        <FormItem v-if="templateMode" label="模板名称" required>
          <BaseInput v-model="templateName" maxlength="128" placeholder="模板名称" />
        </FormItem>
        <FormItem label="检测模式" required>
          <BaseSelect v-model="draft.detectionMode" :options="[{ value: 'answer', label: '题目检测' }, { value: 'state_probe', label: '状态探针' }]" />
        </FormItem>
        <div v-if="!templateMode && editing" class="grid gap-1 text-cp-sm">
          <span class="font-medium">被测账号</span>
          <span class="break-all text-cp-text-secondary">{{ accountName(draft.accountId) }}</span>
        </div>
        <div v-else-if="!templateMode" class="grid gap-2">
          <QualityCatalogPicker v-model:selected-values="selectedAccounts" multiple label="被测账号" search-placeholder="搜索账号名称或邮箱" :load-page="accountPage" />
          <p class="text-xs text-cp-text-secondary">
            每个账号单独建立一条规则，已有规则的账号不会被覆盖。
          </p>
        </div>
        <div class="grid gap-4 sm:grid-cols-2">
          <FormItem label="检测模型" required>
            <BaseInput v-model="draft.model" placeholder="模型 ID" />
          </FormItem>
          <FormItem v-if="!isProbe" label="推理强度">
            <BaseSelect v-model="effort" :options="[{ value: '', label: '按默认' }, ...efforts]" />
          </FormItem>
          <QualitySchedule v-model="draft.intervalSeconds" :cron="draft.cron" :timezone="draft.timezone" />
          <div v-if="!isProbe" class="grid gap-2 text-cp-sm">
            <span>每轮并行答题次数</span><BaseNumberInput v-model="draft.repetitions" label="每轮并行答题次数" :min="1" :max="8" />
            <span class="text-xs text-cp-text-secondary">每轮并行检测 {{ draft.repetitions }} 次，不受业务并发上限限制，仍遵守账号请求间隔及安全策略。</span>
          </div>
          <BaseSwitch v-model="draft.enabled" label="启用定时检测" show-label />
          <p class="text-xs text-cp-text-secondary sm:col-span-2">
            关闭后不会定时或手动检测；保存规则不会立即消耗额度。
          </p>
        </div>
        <FormItem v-if="!isProbe" label="题目" required>
          <BaseTextarea v-model="draft.prompt" :rows="5" />
        </FormItem>
        <FormItem v-if="!isProbe" label="参考答案" required>
          <BaseTextarea v-model="draft.referenceAnswer" :rows="3" />
        </FormItem>
        <div v-if="!isProbe" class="border-t border-cp-border pt-4">
          <h3 class="mb-3 font-semibold">
            答案判定
          </h3>
          <div class="grid gap-4">
            <p class="text-xs text-cp-text-secondary">
              从所选分组调度其他账号，用判题模型对照参考答案评分；不会让被测账号给自己判题。
            </p>
            <QualityCatalogPicker v-model="draft.judgeGroupId" label="判题账号分组" search-placeholder="搜索分组名称" :load-page="groupPage" />
            <FormItem label="判题模型" required>
              <BaseInput v-model="draft.judgeModel" placeholder="模型 ID" />
            </FormItem>
            <FormItem label="判题提示词" required>
              <BaseTextarea v-model="draft.judgePrompt" :rows="3" />
            </FormItem>
          </div>
        </div>
        <div class="grid gap-4 border-t border-cp-border pt-4">
          <h3 class="font-semibold">
            {{ isProbe ? '异常后的处理' : '答错后的处理' }}
          </h3>
          <FormItem label="处理方式">
            <BaseSelect v-model="draft.failureAction" :options="failureActionOptions(draft.failureAction)" />
          </FormItem>
          <QualityCatalogPicker v-if="draft.failureAction === 'remove_groups'" v-model:selected-values="draft.failureGroupIds" multiple label="处置分组" :load-page="actionGroupPage" />
          <AccountTemplatePicker v-if="draft.failureAction === 'apply_account_template'" v-model="selectedTemplate" label="异常处置账号模板" :disabled="busy" />
          <p v-if="draft.failureAction === 'apply_account_template'" class="text-xs text-cp-text-secondary">
            达到阈值后完整应用模板，包含调度、并发、权重、分组、代理及所选 Excel/IPv6 设置；Excel 是否开启由模板决定。模板修改后需重新选择确认。
          </p>
          <div v-if="usesFailureThreshold(draft.failureAction)" class="grid gap-2 text-cp-sm">
            <span>连续异常多少轮后执行处置</span>
            <BaseNumberInput v-model="draft.excelFailureThreshold" label="连续异常阈值" :min="1" :max="100" />
            <p class="text-xs text-cp-text-secondary">
              明确异常每轮计 1 次，正常轮清零，无法判断或请求失败不累计也不清零。保存规则会清零计数。不会自动撤销已应用的配置。
            </p>
          </div>
          <BaseSwitch v-if="!usesFailureThreshold(draft.failureAction)" v-model="draft.autoRestore" label="后续整轮通过后自动恢复" show-label />
          <p class="text-xs text-cp-text-secondary">
            {{ isProbe ? '无法判断时不执行处置。换票结果仅表示探针观察，不等于模型能力的完整评估。' : '仅明确答错触发处置，网络或判题错误不算降质。只撤销本规则的改动，不解除其他停用原因。' }}
          </p>
        </div>
      </form>
      <template #footer>
        <BaseButton :disabled="busy" @click="editorOpen = false">
          取消
        </BaseButton>
        <BaseButton form="quality-rule-form" type="submit" :disabled="busy || (templateMode ? !templateName.trim() : editing ? !draft.accountId : !selectedAccounts.length) || (!isProbe && !draft.judgeGroupId) || (draft.failureAction === 'remove_groups' && !draft.failureGroupIds.length) || (draft.failureAction === 'apply_account_template' && !draft.failureTemplate)">
          <Save class="size-4" />保存
        </BaseButton>
      </template>
    </QualityDrawer>
    <QualityBulkEditor v-model="bulkOpen" :rules="rules" :selected-ids="batchSelection" :default-config="defaults()" :account-name="accountName" :group-page="groupPage" :action-group-page="actionGroupPage" @busy="busy = $event" @saved="selectBatchRule($event, false)" @finished="load" />
    <QualityDrawer v-model="detailOpen" title="检测详情">
      <p v-if="detailError" role="alert" class="text-cp-error">
        {{ detailError }}
      </p>
      <p v-else-if="!detail" class="text-cp-text-secondary">
        加载中…
      </p>
      <div v-else class="grid min-w-0 gap-4">
        <div class="grid gap-1 border-b border-cp-border pb-4 text-cp-sm">
          <span class="break-all">{{ accountName(detail.accountId) }}</span>
          <span class="break-all font-mono">{{ detail.model }}</span>
          <span>{{ formatDateTime(detail.startedAt) }}</span>
          <span>{{ modeLabel(detail.detectionMode) }}</span>
          <strong :class="statusColor(detail.status)">{{ verdictLabel(detail.status, detail.detectionMode) }}</strong>
          <span v-if="detail.action">{{ actions[detail.action] || detail.action }}</span>
          <span v-if="detail.config?.failureTemplate" class="break-words">
            本轮处置模板：{{ detail.config.failureTemplate.config.name }}（版本 {{ detail.config.failureTemplate.revision }}）
          </span>
        </div>
        <details v-if="detail.config && detail.detectionMode !== 'state_probe'" class="min-w-0 border-b border-cp-border pb-4 text-cp-sm">
          <summary class="cursor-pointer font-semibold">
            本轮题目与参考答案
          </summary>
          <p class="mt-3 whitespace-pre-wrap break-words [overflow-wrap:anywhere]">
            {{ detail.config.prompt }}
          </p>
          <p class="mt-3 whitespace-pre-wrap break-words [overflow-wrap:anywhere]">
            {{ detail.config.referenceAnswer }}
          </p>
          <p class="mt-3 break-all text-cp-text-secondary">
            判题模型 {{ detail.config.judgeModel }}
          </p>
        </details>
        <section v-for="answer in detail.answers ?? []" :key="answer.index" class="min-w-0 border-b border-cp-border pb-4">
          <div class="mb-2 flex flex-wrap gap-3 text-cp-sm">
            <strong>第 {{ answer.index }} 次</strong><span :class="statusColor(answer.verdict)">{{ verdictLabel(answer.verdict, detail.detectionMode) }}</span><span>{{ (answer.elapsedMs / 1000).toFixed(1) }} 秒</span>
          </div>
          <p v-if="answer.returnedModel" class="mb-2 break-all text-xs text-cp-text-secondary">
            返回模型 {{ answer.returnedModel }}
          </p>
          <dl v-if="answer.probe" class="grid gap-2 text-cp-sm">
            <div v-for="(shot, index) in answer.probe.shots" :key="index" class="flex flex-wrap gap-x-3 gap-y-1">
              <dt class="font-medium">
                第 {{ index + 1 }} 步
              </dt>
              <dd>{{ shot.transport || '链路未确认' }}</dd>
              <dd v-if="shot.status">
                HTTP {{ shot.status }}
              </dd>
              <dd>票据长度 {{ shot.ticketLength }}</dd>
              <dd v-if="shot.changed !== null">
                {{ shot.changed ? '票据已变化' : '未观察到换票' }}
              </dd>
            </div>
          </dl>
          <pre v-else class="max-h-80 overflow-auto whitespace-pre-wrap break-words text-cp-sm [overflow-wrap:anywhere]">{{ answer.answer || '无回答' }}</pre>
          <p class="mt-3 whitespace-pre-wrap break-words text-cp-sm [overflow-wrap:anywhere]">
            {{ answer.reason }}
          </p>
          <p v-if="answer.judgeAccountId" class="mt-2 break-all text-xs text-cp-text-secondary">
            判题账号 {{ accountName(answer.judgeAccountId) }}
          </p>
        </section>
      </div>
    </QualityDrawer>
    <BaseConfirmModal v-model="deleteOpen" title="删除检测规则" destructive :loading="busy" @confirm="confirmDelete">
      删除此账号的检测规则及其历史记录？此前暂停的调度或移出的分组不会自动恢复。
    </BaseConfirmModal>
    <BaseConfirmModal v-model="deleteTemplateOpen" title="删除规则模板" destructive :loading="busy" @confirm="confirmDeleteTemplate">
      删除「{{ deleteTemplateTarget?.name }}」？已应用到账号的规则和检测历史会保留。
      <p v-if="templatesError" class="mt-2 text-cp-error" role="alert">
        {{ templatesError }}
      </p>
    </BaseConfirmModal>
  </div>
</template>

<style scoped>
.quality-header {
  min-height: 44px;
}
.quality-header :deep(h1) {
  font-size: 26px;
  line-height: 1.25;
  letter-spacing: 0;
}
.quality-rule-selected {
  border-inline-start: 3px solid var(--cp-color-primary);
}
</style>
