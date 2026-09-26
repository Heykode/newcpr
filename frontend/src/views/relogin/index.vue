<script setup lang="ts">
import type { AccountTemplate } from '@/api/modules/account-templates'
import type { ReloginBatchResult, ReloginEntry, ReloginPushSelection, ReloginWorkspaceMode } from '@/api/modules/relogin'
import { CheckCheck, GripVertical, LayoutTemplate, Pause, Play, RefreshCw, Save, Search, Settings2, Trash2, Upload, X } from '@lucide/vue'
import { useNow } from '@vueuse/core'
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import {
  configureRelogin,
  deleteRelogin,
  getRelogin,
  importRelogin,
  pushRelogin,
  queueRelogin,
  resumeReloginWorkspace,
  setReloginAutomatic,
  setReloginWorkspace,
} from '@/api/modules/relogin'
import AccountTemplatePicker from '@/components/account-templates/AccountTemplatePicker.vue'
import AccountTemplatesModal from '@/components/account-templates/AccountTemplatesModal.vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import { defineTableColumns } from '@/components/base/BaseTable/columns'
import BaseTable from '@/components/base/BaseTable/index.vue'
import BaseTextarea from '@/components/base/BaseTextarea.vue'
import { toast } from '@/components/base/BaseToast'
import ExcelModelFields from '@/components/ExcelModelFields.vue'
import ReloginCountCell from '@/components/ReloginCountCell.vue'
import { normalizeAccountName } from '@/utils/account-name'
import { errorMessage } from '@/utils/async'
import { formatDateTime } from '@/utils/date'
import { DEFAULT_EXCEL_MODELS_INPUT } from '@/utils/excel-defaults'
import { excelSettings } from '@/utils/excel-settings'
import { useAccountSwipeSelect } from '../accounts/composables/useAccountSwipeSelect'
import { importPreview } from './import-preview'
import { credentialLabel, matchesPool, poolPresentation, processingStatus, recoveryCountdown, recoveryLabels, retryProgress, shortWorkspace, statusLabels, workspaceChoices, workspaceId } from './presentation'

const entries = shallowRef<ReloginEntry[]>([])
const loading = shallowRef(false)
const busy = shallowRef(false)
const failure = shallowRef('')
const loadFailure = shallowRef('')
const selected = ref(new Set<string>())
const search = shallowRef('')
const plan = shallowRef('')
const status = shallowRef('')
const pool = shallowRef('')
const automatic = shallowRef('')
const page = shallowRef(1)
const pageSize = shallowRef(20)
const concurrency = shallowRef('1')
const savedConcurrency = shallowRef(1)
const paused = shallowRef(false)
const retrySettingsOpen = shallowRef(false)
const maxRetries = shallowRef('2')
const retryIntervalMinutes = shallowRef('5')
const savedRetrySettings = shallowRef({ maxRetries: 2, retryIntervalMinutes: 5 })
const retrySettingsError = shallowRef('')
const now = useNow({ interval: 1000 })
const statusOptions = [{ value: '', label: '全部处理状态' }, ...Object.entries({ ...statusLabels, ...recoveryLabels }).map(([value, label]) => ({ value, label })), { value: 'synced', label: '已同步到号池' }]
const poolOptions = [
  { value: '', label: '全部号池状态' },
  { value: 'absent', label: '未入池' },
  { value: 'present', label: '已在池中' },
  { value: 'normal', label: '正常' },
  { value: 'error', label: '凭据异常' },
  { value: 'disabled', label: '暂停调度' },
  { value: 'rate_limited', label: '请求限流' },
  { value: 'quota_exhausted', label: '额度耗尽' },
]
const autoOptions = [{ value: '', label: '全部自动重登' }, { value: 'on', label: '允许自动重登' }, { value: 'off', label: '关闭自动重登' }]
const plans = computed(() => [{ value: '', label: '全部 PLAN' }, ...[...new Set(entries.value.map(row => row.planType).filter((value): value is string => !!value))].sort().map(value => ({ value, label: value.toUpperCase() }))])
const filtered = computed(() => entries.value.filter(row =>
  row.email.toLowerCase().includes(search.value.trim().toLowerCase())
  && (!plan.value || row.planType === plan.value)
  && (!status.value || processingStatus(row).key === status.value)
  && matchesPool(row, pool.value)
  && (!automatic.value || row.automatic === (automatic.value === 'on')),
))
const visible = computed(() => filtered.value.slice((page.value - 1) * pageSize.value, page.value * pageSize.value))
const allSelected = computed(() => visible.value.length > 0 && visible.value.every(row => selected.value.has(row.id)))
const partialSelected = computed(() => !allSelected.value && visible.value.some(row => selected.value.has(row.id)))
const columns = defineTableColumns<ReloginEntry>([
  { key: 'selection', kind: 'selection' },
  { key: 'identity', label: '账号', kind: 'identity', size: 'xl', grow: 1 },
  { key: 'plan', label: 'PLAN / 工作区', kind: 'custom', size: 'lg' },
  { key: 'status', label: '处理状态', kind: 'status', size: 'lg' },
  { key: 'credential', label: '本次凭据', kind: 'status' },
  { key: 'pool', label: '号池状态', kind: 'status' },
  { key: 'reloginCount', label: '重登次数', kind: 'numeric', size: 'sm', align: 'center' },
  { key: 'automatic', label: '自动重登', kind: 'status', size: 'sm' },
  { key: 'actions', label: '操作', kind: 'actions', size: 'lg' },
])
const table = shallowRef<{ getScrollElement: () => HTMLElement | undefined, getTableElement: () => HTMLTableElement | undefined }>()
const rowIds = computed(() => visible.value.map(row => row.id))
const { onMouseDown, overlayStyle } = useAccountSwipeSelect({
  getScrollElement: () => table.value?.getScrollElement(),
  getTableElement: () => table.value?.getTableElement(),
  rowIds,
  selectedIds: selected,
  disabled: busy,
})
let timer: ReturnType<typeof setTimeout> | undefined
let disposed = false
let readController: AbortController | undefined
let generation = 0

async function reload(silent = false) {
  readController?.abort()
  const controller = new AbortController()
  readController = controller
  const owner = ++generation
  loading.value = true
  try {
    const result = await getRelogin({ silent: true, signal: controller.signal })
    if (disposed || owner !== generation)
      return
    entries.value = result.items
    paused.value = result.settings.paused
    if (concurrency.value === String(savedConcurrency.value))
      concurrency.value = String(result.settings.concurrency)
    savedConcurrency.value = result.settings.concurrency
    savedRetrySettings.value = {
      maxRetries: result.settings.maxRetries ?? 2,
      retryIntervalMinutes: result.settings.retryIntervalMinutes ?? 5,
    }
    const ids = new Set(result.items.map(row => row.id))
    const retainedSelection = new Set([...selected.value].filter(id => ids.has(id)))
    if (retainedSelection.size !== selected.value.size)
      selected.value = retainedSelection
    loadFailure.value = ''
  }
  catch (error) {
    if (!controller.signal.aborted && !disposed && owner === generation) {
      loadFailure.value = errorMessage(error)
      if (!silent)
        toast.error(loadFailure.value)
    }
  }
  finally {
    if (owner === generation)
      loading.value = false
  }
}

async function poll() {
  if (disposed)
    return
  if (!busy.value && !loading.value && !document.hidden)
    await reload(true)
  if (!disposed)
    timer = setTimeout(poll, 5000)
}

async function action(operation: () => Promise<unknown>) {
  if (busy.value)
    return
  busy.value = true
  failure.value = ''
  try {
    await operation()
  }
  catch (error) { failure.value = errorMessage(error) }
  finally {
    busy.value = false
    if (!disposed)
      await reload(true)
  }
}

function toggle(id: string, checked: boolean) {
  const next = new Set(selected.value)
  if (checked)
    next.add(id)
  else next.delete(id)
  selected.value = next
}
function selectPage(checked: boolean) {
  const next = new Set(selected.value)
  for (const row of visible.value) {
    if (checked)
      next.add(row.id)
    else next.delete(row.id)
  }
  selected.value = next
}
function batchReport(results: ReloginBatchResult[]) {
  const failed = results.filter(item => !item.success)
  if (failed.length) {
    failure.value = failed.map(item => `${entries.value.find(row => row.id === item.id)?.email ?? item.id}：${item.message}`).join('\n')
    toast.warning(`${results.length - failed.length} 项完成，${failed.length} 项未完成`)
  }
  else {
    toast.success(`${results.length} 项完成`)
  }
}
const queueOpen = shallowRef(false)
const queuedIds = shallowRef<string[]>([])
const queueMode = shallowRef<ReloginWorkspaceMode>('highest')
const queueOptions = [{ value: 'highest', label: '最高套餐' }, { value: 'original', label: '原工作区' }]
function queue(ids: string[]) {
  queuedIds.value = [...ids]
  queueMode.value = 'highest'
  queueOpen.value = true
}
function executeQueue() {
  return action(async () => {
    batchReport(await queueRelogin(queuedIds.value, queueMode.value))
    queueOpen.value = false
  })
}
function changeAutomatic(ids: string[], enabled: boolean) {
  return action(() => setReloginAutomatic(ids, enabled))
}
function configure(nextPaused = paused.value) {
  const value = Number(concurrency.value)
  if (!Number.isInteger(value) || value < 1 || value > 8) {
    toast.warning('并发必须为 1 至 8')
    return
  }
  return action(async () => {
    await configureRelogin({ concurrency: value, paused: nextPaused })
    savedConcurrency.value = value
    paused.value = nextPaused
  })
}
function openRetrySettings() {
  maxRetries.value = String(savedRetrySettings.value.maxRetries)
  retryIntervalMinutes.value = String(savedRetrySettings.value.retryIntervalMinutes)
  retrySettingsError.value = ''
  retrySettingsOpen.value = true
}
function saveRetrySettings() {
  const retries = Number(maxRetries.value)
  const interval = Number(retryIntervalMinutes.value)
  if (!maxRetries.value.trim() || !Number.isInteger(retries) || retries < 0 || retries > 10) {
    retrySettingsError.value = '失败重试次数必须为 0 至 10'
    return
  }
  if (!retryIntervalMinutes.value.trim() || !Number.isInteger(interval) || interval < 1 || interval > 1440) {
    retrySettingsError.value = '重试间隔必须为 1 至 1440 分钟'
    return
  }
  retrySettingsError.value = ''
  return action(async () => {
    try {
      await configureRelogin({
        concurrency: savedConcurrency.value,
        paused: paused.value,
        maxRetries: retries,
        retryIntervalMinutes: interval,
      })
      savedRetrySettings.value = { maxRetries: retries, retryIntervalMinutes: interval }
      retrySettingsOpen.value = false
      toast.success('自动重登设置已保存')
    }
    catch (error) {
      retrySettingsError.value = errorMessage(error)
      throw error
    }
  })
}
const importing = shallowRef(false)
const importText = shallowRef('')
const replaceExisting = shallowRef(false)
const fileInput = shallowRef<HTMLInputElement>()
const preview = computed(() => importPreview(importText.value))
async function readFile(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (!file)
    return
  if (file.size > 512 * 1024) {
    toast.warning('文件不能超过 512 KiB')
    return
  }
  try {
    const text = await file.text()
    if (importing.value)
      importText.value = text
  }
  catch { toast.error('文件读取失败') }
}
function saveImport() {
  return action(async () => {
    const result = await importRelogin(importText.value, replaceExisting.value)
    importing.value = false
    importText.value = ''
    page.value = 1
    toast.success(`已导入 ${result.imported} 个账号`)
  })
}
const confirming = shallowRef(false)
const templatesOpen = shallowRef(false)
const selectedTemplate = shallowRef<AccountTemplate | null>(null)
const batchCustomName = shallowRef('')
const applyExcel = shallowRef(false)
const excelEnabled = shallowRef(false)
const excelCacheCreationAsInput = shallowRef(false)
const excelAutoDisableOn403 = shallowRef(false)
const excelModelsFollowGlobal = shallowRef(true)
const excelModels = shallowRef(DEFAULT_EXCEL_MODELS_INPUT)
const confirmMode = shallowRef<'push' | 'delete'>('push')
const pendingRows = shallowRef<ReloginEntry[]>([])
const pushAccounts = ref<Record<string, string>>({})
function confirm(mode: 'push' | 'delete', ids: string[]) {
  failure.value = ''
  selectedTemplate.value = null
  batchCustomName.value = ''
  applyExcel.value = false
  excelEnabled.value = false
  excelCacheCreationAsInput.value = false
  excelAutoDisableOn403.value = false
  excelModelsFollowGlobal.value = true
  excelModels.value = DEFAULT_EXCEL_MODELS_INPUT
  confirmMode.value = mode
  pendingRows.value = entries.value.filter(row => ids.includes(row.id)).map(row => ({ ...row }))
  pushAccounts.value = Object.fromEntries(pendingRows.value.map((row) => {
    const targets = row.pushTargets?.filter(target => target.available) ?? []
    return [row.id, targets.length === 1 ? targets[0]!.accountId : '']
  }))
  confirming.value = pendingRows.value.length > 0
}
function canPush(row: ReloginEntry) {
  return row.status === 'ready' && row.credentialStatus === 'verified' && row.poolStatus !== 'synced'
}
const pushable = computed(() => pendingRows.value.filter(canPush))
const newPushCount = computed(() => pushable.value.filter(row => row.poolAccountIds.length === 0).length)
function requiresPushTarget(row: ReloginEntry) {
  return row.workspaceMode === 'highest' && (row.pushTargets?.length ?? 0) > 0
}
function pushTarget(row: ReloginEntry) {
  return row.pushTargets?.find(target => target.available && target.accountId === pushAccounts.value[row.id])
}
function pushTargetOptions(row: ReloginEntry) {
  return (row.pushTargets ?? []).filter(target => target.available).map(target => ({
    value: target.accountId,
    label: `${target.planType?.toUpperCase() ?? '未知套餐'} · ${target.workspaceId}`,
  }))
}
const pushSelectionMissing = computed(() => pushable.value.some(row =>
  requiresPushTarget(row) && !pushTarget(row),
))
function executeConfirmed() {
  if (confirmMode.value === 'push' && pushSelectionMissing.value)
    return
  return action(async () => {
    if (confirmMode.value === 'push') {
      const template = newPushCount.value > 0 && selectedTemplate.value
        ? { id: selectedTemplate.value.id, revision: selectedTemplate.value.revision }
        : undefined
      const customName = newPushCount.value > 0 ? normalizeAccountName(batchCustomName.value) ?? undefined : undefined
      const selections: Record<string, ReloginPushSelection> = {}
      for (const row of pushable.value) {
        const target = pushTarget(row)
        if (requiresPushTarget(row) && target)
          selections[row.id] = { accountId: target.accountId, switchWorkspace: target.switchWorkspace }
      }
      const newAccountExcel = newPushCount.value > 0 && applyExcel.value
        ? excelSettings(excelEnabled.value, excelModelsFollowGlobal.value, excelModels.value, excelCacheCreationAsInput.value, excelAutoDisableOn403.value)
        : undefined
      batchReport(await pushRelogin(pushable.value, template, customName, Object.keys(selections).length ? selections : undefined, newAccountExcel))
    }
    else {
      await deleteRelogin(pendingRows.value.map(row => row.id))
      toast.success('重登资料已删除，号池账号保持不变')
    }
    confirming.value = false
  })
}
const editing = shallowRef(false)
const editRow = shallowRef<ReloginEntry>()
const workspace = shallowRef('')
const workspaceError = shallowRef('')
const awaitingWorkspace = computed(() => editRow.value?.status === 'awaiting_workspace')
const workspaceStale = computed(() => !!editRow.value
  && entries.value.find(row => row.id === editRow.value?.id)?.revision !== editRow.value.revision)
const workspaceOptions = computed(() => [
  ...awaitingWorkspace.value ? [] : [{ value: '', label: '自动选择（已有账号沿用原工作区）' }],
  ...editRow.value ? workspaceChoices(editRow.value) : [],
])
const workspaceKnown = computed(() => workspaceOptions.value.some(option => option.value === workspace.value))
const selectedWorkspace = computed(() => editRow.value?.workspaceChoices?.find(choice => choice.id === workspace.value))
function edit(row: ReloginEntry) {
  editRow.value = row
  workspaceError.value = ''
  workspace.value = row.status === 'awaiting_workspace' ? '' : row.preferredWorkspaceId ?? ''
  editing.value = true
}
function saveWorkspace() {
  const row = editRow.value
  if (!row)
    return
  if (!workspaceKnown.value || workspaceStale.value)
    return
  return action(async () => {
    workspaceError.value = ''
    try {
      if (row.status === 'awaiting_workspace')
        await resumeReloginWorkspace(row.id, row.revision, workspace.value)
      else
        await setReloginWorkspace(row.id, workspace.value.trim() || null)
      editing.value = false
    }
    catch (error) {
      workspaceError.value = errorMessage(error)
      throw error
    }
  })
}
watch([search, plan, status, pool, automatic, pageSize], () => {
  page.value = 1
})
watch(() => filtered.value.length, (length) => {
  page.value = Math.min(page.value, Math.max(1, Math.ceil(length / pageSize.value)))
})
watch(importing, (open) => {
  if (!open) {
    importText.value = ''
    replaceExisting.value = false
  }
})
onMounted(async () => {
  await reload()
  if (!disposed)
    timer = setTimeout(poll, 5000)
})
onBeforeUnmount(() => {
  disposed = true
  generation++
  readController?.abort()
  clearTimeout(timer)
  importText.value = ''
})
</script>

<template>
  <div class="flex h-full min-h-0 min-w-0 flex-col gap-4 overflow-hidden">
    <header class="flex shrink-0 flex-wrap items-center justify-between gap-3">
      <div class="flex items-center gap-2">
        <h1 class="m-0 text-xl font-bold text-cp-text">
          失效重登
        </h1>
        <BaseIconButton label="刷新重登列表" :loading="loading" :disabled="busy" @click="reload()">
          <RefreshCw class="size-4" />
        </BaseIconButton>
      </div>
      <div class="flex flex-wrap items-center gap-2">
        <span class="text-cp-sm text-cp-text-secondary">并发</span>
        <BaseInput v-model="concurrency" class="w-18" type="number" min="1" max="8" step="1" aria-label="重登并发" :disabled="busy" />
        <BaseIconButton label="保存并发设置" :disabled="busy || concurrency === String(savedConcurrency)" @click="configure()">
          <Save class="size-4" />
        </BaseIconButton>
        <BaseIconButton :label="paused ? '恢复重登队列' : '暂停重登队列'" :disabled="busy" @click="configure(!paused)">
          <Play v-if="paused" class="size-4 text-cp-success" /><Pause v-else class="size-4" />
        </BaseIconButton>
        <BaseIconButton label="自动重登设置" :disabled="busy" @click="openRetrySettings">
          <Settings2 class="size-4" />
        </BaseIconButton>
        <BaseButton variant="primary" :disabled="busy" @click="importing = true">
          <template #icon>
            <Upload class="size-4" />
          </template>导入
        </BaseButton>
        <BaseButton :disabled="busy" @click="templatesOpen = true">
          <template #icon>
            <LayoutTemplate class="size-4" />
          </template>账号模板
        </BaseButton>
      </div>
    </header>
    <div v-if="paused" class="text-cp-sm text-cp-warning" role="status">
      重登队列已暂停
    </div>
    <div class="grid shrink-0 grid-cols-2 gap-2 lg:grid-cols-[minmax(180px,1fr)_repeat(4,minmax(140px,180px))]">
      <BaseInput v-model="search" class="col-span-2 lg:col-span-1" aria-label="搜索邮箱" placeholder="搜索邮箱">
        <template #prefix>
          <Search class="size-4" />
        </template>
      </BaseInput>
      <BaseSelect v-model="plan" :options="plans" aria-label="PLAN 筛选" />
      <BaseSelect v-model="status" :options="statusOptions" aria-label="处理状态筛选" />
      <BaseSelect v-model="pool" :options="poolOptions" aria-label="号池状态筛选" />
      <BaseSelect v-model="automatic" :options="autoOptions" aria-label="自动重登筛选" />
    </div>
    <div class="flex shrink-0 flex-wrap items-center gap-2 border-y border-cp-border py-2">
      <span class="text-cp-sm text-cp-text-secondary">已选 {{ selected.size }} 项</span>
      <BaseIconButton label="取消选择" :disabled="busy || !selected.size" @click="selected = new Set()">
        <X class="size-4" />
      </BaseIconButton>
      <BaseButton :disabled="busy || !selected.size || selected.size > 500" @click="queue([...selected])">
        <template #icon>
          <RefreshCw class="size-4" />
        </template>批量重登
      </BaseButton>
      <BaseButton :disabled="busy || !selected.size || selected.size > 500" @click="confirm('push', [...selected])">
        <template #icon>
          <Upload class="size-4" />
        </template>批量推送
      </BaseButton>
      <BaseIconButton label="批量开启自动重登" :disabled="busy || !selected.size || selected.size > 500" @click="changeAutomatic([...selected], true)">
        <CheckCheck class="size-4 text-cp-success" />
      </BaseIconButton>
      <BaseIconButton label="批量关闭自动重登" :disabled="busy || !selected.size || selected.size > 500" @click="changeAutomatic([...selected], false)">
        <Pause class="size-4" />
      </BaseIconButton>
      <BaseIconButton label="批量删除资料" :disabled="busy || !selected.size || selected.size > 500" @click="confirm('delete', [...selected])">
        <Trash2 class="size-4 text-cp-error" />
      </BaseIconButton>
    </div>
    <div v-if="failure || loadFailure" class="max-h-24 shrink-0 overflow-auto whitespace-pre-wrap break-words text-cp-sm text-cp-error" role="alert">
      {{ failure || loadFailure }}
    </div>
    <section class="flex min-h-48 min-w-0 flex-1 flex-col overflow-hidden border-y border-cp-border bg-cp-bg-container">
      <BaseTable ref="table" class="relogin-table min-h-0 flex-1" :columns="columns" :rows="visible" :selected-row-keys="[...selected]" :loading="loading && !entries.length" column-layout="content" empty-text="暂无匹配账号" @mousedown="onMouseDown">
        <template #header-selection>
          <BaseCheckbox label="全选当前页" :model-value="allSelected" :indeterminate="partialSelected" :disabled="busy" @update:model-value="selectPage" />
        </template>
        <template #selection="{ row }">
          <BaseCheckbox :label="`选择 ${row.email}`" :model-value="selected.has(row.id)" :disabled="busy" @update:model-value="toggle(row.id, $event)" />
        </template>
        <template #identity="{ row }">
          <div class="flex min-w-0 items-center gap-2">
            <span data-swipe-select-handle class="cursor-ns-resize p-1 text-cp-text-tertiary" title="拖动选择"><GripVertical class="size-4" /></span>
            <div class="min-w-0" data-swipe-select-handle>
              <div class="truncate font-semibold text-cp-text" :title="row.email">
                {{ row.email }}
              </div>
              <div class="truncate text-cp-xs text-cp-text-tertiary" :title="row.message">
                {{ row.message || '待处理' }}
              </div>
              <div class="truncate text-cp-xs text-cp-text-tertiary">
                导入 {{ row.importedAt ? formatDateTime(row.importedAt) : '时间未记录' }}
              </div>
            </div>
          </div>
        </template>
        <template #plan="{ row }">
          <div class="w-full min-w-0 overflow-hidden">
            <div class="truncate font-mono text-cp-sm" :title="row.planType?.toUpperCase()">
              {{ row.planType?.toUpperCase() ?? '待识别' }}
            </div>
            <div class="truncate font-mono text-cp-xs text-cp-text-tertiary" :title="workspaceId(row) ?? '自动选择工作区'">
              {{ shortWorkspace(workspaceId(row)) }}
            </div>
          </div>
        </template>
        <template #status="{ row }">
          <button v-if="row.status === 'awaiting_workspace'" type="button" class="block cursor-pointer border-0 bg-transparent p-0 text-left text-cp-sm text-cp-warning underline underline-offset-4 disabled:cursor-not-allowed disabled:opacity-50" :disabled="busy || paused" @click="edit(row)">
            待选择工作区
          </button>
          <span v-else class="block whitespace-normal break-words" :class="processingStatus(row).tone" :title="processingStatus(row).detail">{{ processingStatus(row).label }}</span>
          <span v-if="retryProgress(row)" class="block text-cp-xs tabular-nums text-cp-text-tertiary">{{ retryProgress(row) }}</span>
          <span v-if="processingStatus(row).key === 'manual_required'" class="block whitespace-normal break-words text-cp-xs text-cp-error">{{ row.recovery?.message }}</span>
          <span v-if="row.recovery?.retryAt" class="block whitespace-normal break-words text-cp-xs tabular-nums text-cp-text-tertiary">{{ recoveryCountdown(row, now.getTime()) }}</span>
        </template>
        <template #credential="{ row }">
          <span class="block whitespace-normal break-words" :class="row.credentialStatus === 'verified' ? 'text-cp-success' : row.credentialStatus === 'expired' ? 'text-cp-warning' : 'text-cp-text-tertiary'" :title="row.verifiedAt ? `本次缓存凭据验证于 ${formatDateTime(row.verifiedAt)}；不代表已推送或号池当前正常` : '尚未通过重登获取新的 JSON；与号池已有凭据无关'">{{ credentialLabel(row) }}</span>
        </template>
        <template #pool="{ row }">
          <div :title="poolPresentation(row).detail">
            <span class="block whitespace-normal break-words" :class="poolPresentation(row).tone">{{ poolPresentation(row).label }}</span>
            <span v-if="poolPresentation(row).caption" class="block text-cp-xs text-cp-text-tertiary">{{ poolPresentation(row).caption }}</span>
          </div>
        </template>
        <template #automatic="{ row }">
          <BaseSwitch :label="`${row.email} 自动重登`" :model-value="row.automatic" :disabled="busy" @update:model-value="changeAutomatic([row.id], $event)" />
        </template>
        <template #reloginCount="{ row }">
          <ReloginCountCell :count="row.reloginCount" :last-relogin-at="row.lastReloginAt" />
        </template>
        <template #actions="{ row }">
          <div class="grid w-full min-w-0 justify-items-start gap-1 py-1">
            <div class="flex items-center gap-1">
              <BaseButton class="w-12 whitespace-nowrap px-2!" size="sm" title="重新登录并获取新凭据" :aria-busy="row.status === 'running' || undefined" :disabled="busy || ['queued', 'running', 'pushing'].includes(row.status)" @click="queue([row.id])">
                重登
              </BaseButton>
              <BaseButton class="w-12 whitespace-nowrap px-2!" variant="soft" size="sm" title="确认推送到号池" :disabled="busy || !canPush(row)" @click="confirm('push', [row.id])">
                推送
              </BaseButton>
            </div>
            <div class="flex items-center gap-1">
              <BaseIconButton label="选择工作区" size="sm" :disabled="busy || ['queued', 'running', 'pushing', 'uncertain'].includes(row.status)" @click="edit(row)">
                <Settings2 class="size-3.5" />
              </BaseIconButton>
              <BaseIconButton label="删除重登资料" size="sm" :disabled="busy" @click="confirm('delete', [row.id])">
                <Trash2 class="size-3.5 text-cp-error" />
              </BaseIconButton>
            </div>
          </div>
        </template>
      </BaseTable>
    </section>
    <BaseTablePagination :pagination="{ currentPage: page, pageSize, total: filtered.length }" :loading="busy" @page-change="page = $event" @page-size-change="pageSize = $event" />
    <div v-if="overlayStyle" :style="overlayStyle" class="pointer-events-none fixed z-50 border border-cp-primary bg-cp-primary/10" />

    <BaseModal v-model="retrySettingsOpen" title="自动重登设置" size="sm" :dismissible="!busy">
      <div class="grid min-w-0 gap-4">
        <BaseFormItem label="失败重试次数（不含首次，0 为不重试）">
          <BaseInput v-model="maxRetries" type="number" min="0" max="10" step="1" aria-label="失败重试次数" :disabled="busy" />
        </BaseFormItem>
        <BaseFormItem label="重试间隔（分钟）">
          <BaseInput v-model="retryIntervalMinutes" type="number" min="1" max="1440" step="1" aria-label="重试间隔（分钟）" :disabled="busy" />
        </BaseFormItem>
        <div v-if="retrySettingsError" class="break-words text-cp-sm text-cp-error" role="alert">
          {{ retrySettingsError }}
        </div>
      </div>
      <template #footer>
        <BaseButton :disabled="busy" @click="retrySettingsOpen = false">
          取消
        </BaseButton>
        <BaseButton variant="primary" :loading="busy" @click="saveRetrySettings">
          保存
        </BaseButton>
      </template>
    </BaseModal>
    <BaseModal v-model="importing" title="导入重登资料" size="lg" :dismissible="!busy">
      <div class="grid gap-3">
        <div class="flex items-center justify-between gap-2">
          <span class="text-cp-sm text-cp-text-secondary">邮箱----密码----2FA 密钥</span>
          <BaseButton :disabled="busy" @click="fileInput?.click()">
            <template #icon>
              <Upload class="size-4" />
            </template>选择文件
          </BaseButton>
          <input ref="fileInput" type="file" accept=".txt,.tsv" aria-label="账号资料文件" class="hidden" @change="readFile">
        </div>
        <BaseTextarea v-model="importText" aria-label="账号资料" autocomplete="off" :spellcheck="false" :rows="7" :disabled="busy" />
        <BaseCheckbox v-model="replaceExisting" label="确认更新重复邮箱的密码和 2FA 资料" show-label :disabled="busy" />
        <div v-if="preview.length" class="max-h-40 overflow-auto text-cp-sm">
          <div class="mb-2 text-cp-text-secondary">
            {{ preview.length }} 行 / {{ preview.filter(row => row.valid).length }} 行格式完整
          </div>
          <div v-for="row in preview.slice(0, 500)" :key="row.line" class="flex justify-between gap-3 py-1">
            <span class="min-w-0 truncate">{{ row.line }}. {{ row.email || '未识别邮箱' }}</span><span class="shrink-0" :class="row.valid ? 'text-cp-success' : 'text-cp-error'">{{ row.valid ? '格式正确' : '格式错误' }}</span>
          </div>
        </div>
      </div>
      <template #footer>
        <BaseButton variant="primary" :loading="busy" :disabled="!preview.length || preview.length > 500 || preview.some(row => !row.valid)" @click="saveImport">
          确认导入
        </BaseButton>
      </template>
    </BaseModal>
    <BaseConfirmModal v-model="queueOpen" title="确认重登" :loading="busy" @confirm="executeQueue">
      <BaseFormItem label="本次登录工作区">
        <BaseSelect v-model="queueMode" :options="queueOptions" aria-label="本次登录工作区" :disabled="busy" />
      </BaseFormItem>
      <p class="mb-0 text-cp-sm">
        {{ queuedIds.length }} 个账号，获取后待确认推送。自动重登仍沿用原工作区。
      </p>
    </BaseConfirmModal>
    <BaseConfirmModal v-model="confirming" :title="confirmMode === 'push' ? '确认推送到号池' : '删除重登资料'" :destructive="confirmMode === 'delete'" :loading="busy" :confirm-disabled="confirmMode === 'push' && (!pushable.length || pushSelectionMissing)" @confirm="executeConfirmed">
      <p v-if="failure" class="mt-0 whitespace-pre-wrap break-words text-cp-sm text-cp-error" role="alert">
        {{ failure }}
      </p>
      <p v-if="confirmMode === 'delete'" class="mt-0 text-cp-sm">
        删除 {{ pendingRows.length }} 项资料并取消相关重登任务，号池账号不受影响。
      </p>
      <p v-else class="mt-0 text-cp-sm">
        新增 {{ newPushCount }} 项，更新已有账号 {{ pushable.length - newPushCount }} 项，跳过 {{ pendingRows.length - pushable.length }} 项。
      </p>
      <AccountTemplatePicker v-if="confirming && confirmMode === 'push' && newPushCount > 0" v-model="selectedTemplate" :disabled="busy" />
      <div v-if="confirmMode === 'push' && newPushCount > 0" class="my-4 grid min-w-0 gap-3">
        <BaseCheckbox v-model="applyExcel" label="指定新账号 Excel 设置（优先于模板）" show-label :disabled="busy" />
        <template v-if="applyExcel">
          <div class="flex items-center justify-between gap-3">
            <span class="text-cp-sm">Excel 入口</span>
            <BaseSwitch v-model="excelEnabled" label="切换新账号 Excel 入口" :disabled="busy" />
          </div>
          <BaseFormItem label="Excel 模型">
            <ExcelModelFields v-model:follow-global="excelModelsFollowGlobal" v-model:models="excelModels" :disabled="busy" />
          </BaseFormItem>
          <BaseFormItem label="缓存写入按普通输入计费">
            <BaseSwitch v-model="excelCacheCreationAsInput" label="新账号缓存写入按普通输入计费" :disabled="busy || !excelEnabled" />
          </BaseFormItem>
          <BaseFormItem label="Excel 遇到 HTTP 403 自动关闭">
            <BaseSwitch v-model="excelAutoDisableOn403" label="新账号 Excel 遇到 HTTP 403 自动关闭" :disabled="busy || !excelEnabled" />
          </BaseFormItem>
        </template>
      </div>
      <BaseFormItem v-if="confirmMode === 'push' && newPushCount > 0" label="本批账号名称（选填）">
        <BaseInput v-model="batchCustomName" aria-label="本批账号名称" placeholder="默认名称" :disabled="busy" />
      </BaseFormItem>
      <p v-if="confirmMode === 'push' && pushable.length > newPushCount" class="text-cp-sm text-cp-text-secondary">
        已有账号保留名称、分组、并发、调度及 Excel 配置。
      </p>
      <div class="max-h-64 overflow-auto">
        <div v-for="row in pendingRows" :key="row.id" class="border-b border-cp-border py-2 text-cp-sm">
          <div class="break-all text-cp-text">
            {{ row.email }}
          </div>
          <div v-if="confirmMode === 'push'" class="text-cp-xs">
            {{ row.planType?.toUpperCase() ?? '未知' }} / {{ !canPush(row) ? '不可推送' : row.poolAccountIds.length ? '更新已有账号' : '新增入池' }}
          </div>
          <div v-if="row.workspaceId && confirmMode === 'push'" class="break-all font-mono text-cp-xs">
            {{ row.workspaceId }}
          </div>
          <div v-if="confirmMode === 'push' && canPush(row) && requiresPushTarget(row)" class="mt-2 grid min-w-0 gap-2">
            <BaseSelect v-model="pushAccounts[row.id]" :options="pushTargetOptions(row)" :aria-label="`${row.email} 推送目标`" placeholder="选择要更新的账号" :disabled="busy" />
            <p v-if="pushTarget(row)?.switchWorkspace" class="m-0 break-words text-cp-sm text-cp-warning">
              将原 {{ pushTarget(row)?.planType?.toUpperCase() ?? '未知套餐' }} 账号切换为 {{ row.planType?.toUpperCase() }}，不另建账号。
            </p>
            <p v-else-if="pushTarget(row)" class="m-0 text-cp-sm text-cp-text-secondary">
              更新所选账号，工作区不变。
            </p>
            <p v-else class="m-0 text-cp-sm text-cp-warning">
              {{ pushTargetOptions(row).length ? '请选择推送目标。' : '原账号已变化或目标工作区已存在，请重新获取凭据。' }}
            </p>
          </div>
        </div>
      </div>
    </BaseConfirmModal>
    <AccountTemplatesModal v-model="templatesOpen" />
    <BaseModal v-model="editing" title="选择登录工作区" :dismissible="!busy">
      <div class="grid min-w-0 grid-cols-1 gap-3">
        <p class="m-0 break-all text-cp-sm">
          {{ editRow?.email }}
        </p>
        <BaseSelect v-model="workspace" class="min-w-0" :options="workspaceOptions" placeholder="请选择工作区" aria-label="登录工作区" :disabled="busy" />
        <dl v-if="awaitingWorkspace && selectedWorkspace" class="m-0 grid min-w-0 grid-cols-1 gap-2 text-cp-sm">
          <div>
            <dt class="text-cp-text-secondary">
              工作区
            </dt>
            <dd class="m-0 break-words">
              {{ selectedWorkspace.name || '未命名工作区' }}
            </dd>
          </div>
          <div>
            <dt class="text-cp-text-secondary">
              套餐
            </dt>
            <dd class="m-0">
              {{ selectedWorkspace.planType.toUpperCase() }}
            </dd>
          </div>
          <div>
            <dt class="text-cp-text-secondary">
              工作区 ID
            </dt>
            <dd class="m-0 break-all font-mono text-cp-xs">
              {{ selectedWorkspace.id }}
            </dd>
          </div>
        </dl>
        <p v-if="workspaceStale" class="m-0 text-cp-sm text-cp-warning">
          账号状态已更新，请关闭后重新选择。
        </p>
        <p v-if="workspaceError" role="alert" class="m-0 break-words text-cp-sm text-cp-error">
          {{ workspaceError }}
        </p>
        <p v-else-if="!awaitingWorkspace && !workspaceKnown" class="m-0 text-cp-sm text-cp-warning">
          原指定工作区尚未被识别，请重新选择。
        </p>
        <dl v-if="!awaitingWorkspace" class="m-0 grid gap-2 text-cp-sm text-cp-text-secondary">
          <div class="flex justify-between gap-3">
            <dt>已有号池账号</dt><dd class="m-0">
              沿用选中账号的原工作区
            </dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt>首次获取</dt><dd class="m-0">
              自动优先选择可访问的高套餐
            </dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt>选择变更</dt><dd class="m-0">
              清除缓存凭据，需重新登录
            </dd>
          </div>
        </dl>
      </div>
      <template #footer>
        <BaseButton variant="primary" :loading="busy" :disabled="!workspaceKnown || workspaceStale || (awaitingWorkspace ? paused : workspace === (editRow?.preferredWorkspaceId ?? ''))" @click="saveWorkspace">
          {{ awaitingWorkspace ? '选择并继续' : '保存' }}
        </BaseButton>
      </template>
    </BaseModal>
  </div>
</template>

<style scoped>
@media (max-width: 1599px) {
  .relogin-table :deep([data-column-key='actions']) {
    position: static;
  }
}
</style>
