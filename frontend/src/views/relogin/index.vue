<script setup lang="ts">
import type { EnrollmentEntry } from './enrollment-plan'
import type { ReloginBatchResult, ReloginEntry, ReloginPushSelection, ReloginWorkspaceMode } from '@/api/modules/relogin'
import { CheckCheck, Download, Funnel, GripVertical, MoreHorizontal, Pause, Play, RefreshCw, Save, Search, Settings2, Trash2, Upload, X } from '@lucide/vue'
import { useIntervalFn, useMediaQuery, useNow } from '@vueuse/core'

import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import {
  cancelRelogin,
  configureRelogin,
  deleteRelogin,
  exportRelogin,
  getRelogin,
  importRelogin,
  maintainRelogin,
  pushRelogin,
  queueRelogin,
  resumeReloginWorkspace,
  setReloginAutomatic,
  setReloginWorkspace,
} from '@/api/modules/relogin'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseMenuItem from '@/components/base/BaseMenuItem.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BasePopover from '@/components/base/BasePopover.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import { defineTableColumns } from '@/components/base/BaseTable/columns'
import BaseTable from '@/components/base/BaseTable/index.vue'
import BaseTextarea from '@/components/base/BaseTextarea.vue'
import { toast } from '@/components/base/BaseToast'
import ReloginCountCell from '@/components/ReloginCountCell.vue'
import { useAccountGroupCatalog } from '@/composables/useAccountGroupCatalog'
import { useCopyText } from '@/composables/useCopyText'
import { useDownload } from '@/composables/useDownload'
import { t } from '@/i18n'
import { errorMessage } from '@/utils/async'
import { formatDateTime } from '@/utils/date'
import AccountSetupFields from '../accounts/components/AccountCreateModal/AccountSetupFields.vue'
import { accountCreateEgress, accountImportQualityTemplate, accountImportSettings, accountImportTemplate, accountProxyError, emptyAccountCreateForm } from '../accounts/components/AccountCreateModal/model'
import AccountPlanBadge from '../accounts/components/AccountPlanBadge.vue'
import { useAccountSwipeSelect } from '../accounts/composables/useAccountSwipeSelect'
import EnrollmentPlanModal from './components/EnrollmentPlanModal.vue'
import EnrollmentPlanStatus from './components/EnrollmentPlanStatus.vue'
import ReloginColumnFilter from './components/ReloginColumnFilter.vue'
import ReloginCredentialsModal from './components/ReloginCredentialsModal.vue'
import ReloginEgress from './components/ReloginEgress.vue'
import ReloginFilterOptions from './components/ReloginFilterOptions.vue'
import ReloginHistoryModal from './components/ReloginHistoryModal.vue'
import ReloginMaintenanceModal from './components/ReloginMaintenanceModal.vue'
import ReloginMobileCard from './components/ReloginMobileCard.vue'
import ReloginSessionStatus from './components/ReloginSessionStatus.vue'
import ReloginWorkspaceCard from './components/ReloginWorkspaceCard.vue'
import { canArrange, enrollmentPreview, previewPlanCommand } from './enrollment-plan'
import { importPreview } from './import-preview'
import { knownMaintenanceWorkspaces, maintenanceBusy, maintenanceResult, matchesMaintenanceStatus, queueButtonLabel, queueDisabledReason, recentOperation, recentOperationAt } from './maintenance'
import { matchesPool, processingStatus, recoveryCountdown, recoveryLabels, reloginDisplayName, reloginPlanDisplay, reloginPlanOptions, retryProgress, shortWorkspace, statusLabels, workspaceChoices, workspaceId, workspaceLabel } from './presentation'

const entries = shallowRef<EnrollmentEntry[]>([])
const arranging = shallowRef<EnrollmentEntry | null>(null)
const maintenanceId = shallowRef('')
const maintenanceOpen = shallowRef(false)
const maintenanceTab = shallowRef('session')
const maintenanceEntry = computed(() => entries.value.find(row => row.id === maintenanceId.value) ?? null)
function openMaintenance(row: ReloginEntry, tab = 'session') {
  maintenanceId.value = row.id
  maintenanceTab.value = tab
  maintenanceOpen.value = true
}
async function cancelTask(row: ReloginEntry) {
  try {
    await cancelRelogin(row.id, row.revision)
    await reload(true)
  }
  catch (error) { toast.error(errorMessage(error)) }
}
const summary = computed(() => [
  { key: '', label: '全部账号' },
  { key: 'active', label: '处理中' },
  { key: 'queued', label: '排队' },
  { key: 'needs_attention', label: '待处理' },
  { key: 'failed', label: '异常' },
].map(item => ({ ...item, count: entries.value.filter(row => matchesMaintenanceStatus(row, item.key)).length })))
const webSessionFilter = shallowRef('')
const sessionOptions = [{ label: '全部 Web 会话', value: '' }, { label: '有效', value: 'valid' }, { label: '已失效', value: 'expired' }, { label: '未建立 / 未检测', value: 'unknown' }]
const mobile = useMediaQuery('(max-width: 767px)')
const mobileFiltersOpen = shallowRef(false)
const historyEntry = shallowRef<ReloginEntry | null>(null)
const historyOpen = shallowRef(false)
const credentialsEntry = shallowRef<ReloginEntry | null>(null)
const credentialsOpen = shallowRef(false)
const loading = shallowRef(false)
const busy = shallowRef(false)
const failure = shallowRef('')
const loadFailure = shallowRef('')
const selected = ref(new Set<string>())
const exportOpen = shallowRef(false)
const exportFormat = shallowRef<'json' | 'two_fa'>('json')
const exportIds = shallowRef<string[]>([])
const { downloadText } = useDownload()
const copyText = useCopyText()
async function copySelected() {
  if (busy.value || !selected.value.size || selected.value.size > 200)
    return
  const ids = [...selected.value]
  busy.value = true
  try {
    const result = await exportRelogin(ids, 'two_fa', { silent: true })
    await copyText(result.files.map(file => file.content).join('\n'))
  }
  catch (error) { toast.error(errorMessage(error)) }
  finally { busy.value = false }
}

function confirmExport(format: 'json' | 'two_fa') {
  exportIds.value = [...selected.value]
  exportFormat.value = format
  exportOpen.value = exportIds.value.length > 0 && exportIds.value.length <= 200
}

async function executeExport() {
  if (busy.value)
    return
  busy.value = true
  try {
    const result = await exportRelogin(exportIds.value, exportFormat.value)
    for (const file of result.files)
      await downloadText(file.content, file.name, exportFormat.value === 'json' ? 'application/json;charset=utf-8' : 'text/plain;charset=utf-8')
    exportOpen.value = false
    exportIds.value = []
    toast.success('资料已导出')
  }
  catch (error) { toast.error(errorMessage(error, '导出失败')) }
  finally { busy.value = false }
}
const search = shallowRef('')
const egressFilter = shallowRef('')
const egressOptions = [{ value: '', label: '全部出口' }, { value: 'proxy', label: '账号代理' }, { value: 'ipv6', label: 'IPv6 出口' }, { value: 'default', label: '部署默认出口' }, { value: 'unavailable', label: '出口不可用' }, { value: 'multiple', label: '多个工作区出口' }]
const plan = shallowRef('')
const status = shallowRef('')
const pool = shallowRef('')
const automatic = shallowRef('')
const recent = shallowRef('')
const recentOptions = [{ value: '', label: '全部时间' }, { value: '1', label: '最近 24 小时' }, { value: '7', label: '最近 7 天' }, { value: '30', label: '最近 30 天' }, { value: 'never', label: '尚未操作' }]
const filterCount = computed(() => [plan.value, pool.value, automatic.value, recent.value, status.value, webSessionFilter.value, egressFilter.value].filter(Boolean).length)
const page = shallowRef(1)
const pageSize = shallowRef(20)
const concurrency = shallowRef('1')
const savedConcurrency = shallowRef(1)
const paused = shallowRef(false)
const retrySettingsOpen = shallowRef(false)
const maxRetries = shallowRef('2')
const retryIntervalSeconds = shallowRef('30')
const savedRetrySettings = shallowRef({ maxRetries: 2, retryIntervalSeconds: 30 })
const retrySettingsError = shallowRef('')
const now = useNow({ scheduler: callback => useIntervalFn(callback, 1000) })
const statusOptions = [{ value: '', label: '全部处理状态' }, { value: 'active', label: '处理中' }, { value: 'needs_attention', label: '待处理' }, ...Object.entries({ ...statusLabels, ...recoveryLabels }).map(([value, label]) => ({ value, label })), { value: 'synced', label: '已同步到号池' }]
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
const plans = computed(() => reloginPlanOptions(entries.value))
const filtered = computed(() => entries.value.filter(row =>
  `${reloginDisplayName(row)} ${row.email}`.toLowerCase().includes(search.value.trim().toLowerCase())
  && (!plan.value || reloginPlanDisplay(row) === plan.value)
  && matchesMaintenanceStatus(row, status.value)
  && (!webSessionFilter.value || (webSessionFilter.value === 'unknown' ? !['valid', 'expired'].includes(row.maintenance?.sessionState ?? '') : row.maintenance?.sessionState === webSessionFilter.value))
  && (!recent.value || (recent.value === 'never' ? !recentOperationAt(row) : Date.parse(recentOperationAt(row) ?? '') >= now.value.getTime() - Number(recent.value) * 86400000))
  && (!egressFilter.value || row.egress?.kind === egressFilter.value)
  && matchesPool(row, pool.value)
  && (!automatic.value || row.automatic === (automatic.value === 'on')),
))
const visible = computed(() => filtered.value.slice((page.value - 1) * pageSize.value, page.value * pageSize.value))
const allSelected = computed(() => visible.value.length > 0 && visible.value.every(row => selected.value.has(row.id)))
const partialSelected = computed(() => !allSelected.value && visible.value.some(row => selected.value.has(row.id)))
const columns = defineTableColumns<EnrollmentEntry>([
  { key: 'selection', kind: 'selection' },
  { key: 'identity', label: '账号', kind: 'identity', size: 'xl' },
  { key: 'plan', label: 'PLAN', kind: 'custom', size: 'lg' },
  { key: 'credential', label: '登录状态', kind: 'status', size: 'lg', align: 'left' },
  { key: 'egress', label: '出口 IP', kind: 'custom', size: 'lg' },
  { key: 'status', label: '最近操作', kind: 'custom', size: 'lg' },
  { key: 'automatic', label: '自动恢复', kind: 'status', size: 'md' },
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
      retryIntervalSeconds: result.settings.retryIntervalSeconds ?? 30,
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
const queueEntry = shallowRef<ReloginEntry | null>(null)
const queueWorkspaces = computed(() => knownMaintenanceWorkspaces(queueEntry.value))
const queueDescription = computed(() => {
  const row = queuedIds.value.length === 1 ? entries.value.find(row => row.id === queuedIds.value[0]) : undefined
  return row ? reloginDisplayName(row) : `已选 ${queuedIds.value.length} 个账号`
})
const queueMode = shallowRef('highest')
const queueOptions = [{ value: 'highest', label: '最高套餐', description: '自动选择可用的最高套餐' }, { value: 'original', label: '原工作区', description: '继续使用原账号工作区' }]
function queue(ids: string[]) {
  queuedIds.value = [...ids]
  queueEntry.value = ids.length === 1 ? entries.value.find(row => row.id === ids[0]) ?? null : null
  queueMode.value = 'highest'
  failure.value = ''
  queueOpen.value = true
}
function executeQueue() {
  return action(async () => {
    if (queueMode.value.startsWith('workspace:') && queueEntry.value) {
      await maintainRelogin({ id: queueEntry.value.id, revision: queueEntry.value.revision, action: 'switch_workspace', target: queueMode.value.slice('workspace:'.length) })
      toast.success('已加入重登队列')
    }
    else {
      batchReport(await queueRelogin(queuedIds.value, queueMode.value as ReloginWorkspaceMode))
    }
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
  retryIntervalSeconds.value = String(savedRetrySettings.value.retryIntervalSeconds)
  retrySettingsError.value = ''
  retrySettingsOpen.value = true
}
function saveRetrySettings() {
  const retries = Number(maxRetries.value)
  const interval = Number(retryIntervalSeconds.value)
  if (!maxRetries.value.trim() || !Number.isInteger(retries) || retries < 0 || retries > 10) {
    retrySettingsError.value = '失败重试次数必须为 0 至 10'
    return
  }
  if (!retryIntervalSeconds.value.trim() || !Number.isInteger(interval) || interval < 1 || interval > 86400) {
    retrySettingsError.value = '重试间隔必须为 1 至 86400 秒'
    return
  }
  retrySettingsError.value = ''
  return action(async () => {
    try {
      await configureRelogin({
        concurrency: savedConcurrency.value,
        paused: paused.value,
        maxRetries: retries,
        retryIntervalSeconds: interval,
      })
      savedRetrySettings.value = { maxRetries: retries, retryIntervalSeconds: interval }
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
const pushForm = ref(emptyAccountCreateForm())
const pushFields = ref<InstanceType<typeof AccountSetupFields>>()
const pushProxyError = computed(() => accountProxyError(pushForm.value))
const { groups: pushGroups, loading: pushGroupsLoading, loadGroups: loadPushGroups } = useAccountGroupCatalog({ immediate: false })
const confirmMode = shallowRef<'push' | 'delete'>('push')
const pendingRows = shallowRef<ReloginEntry[]>([])
const pushAccounts = ref<Record<string, string>>({})
function confirm(mode: 'push' | 'delete', ids: string[]) {
  failure.value = ''
  pushForm.value = { ...emptyAccountCreateForm(), provider: 'openai', proxyMode: 'preserve' }
  confirmMode.value = mode
  pendingRows.value = entries.value.filter(row => ids.includes(row.id)).map(row => ({ ...row }))
  pushAccounts.value = Object.fromEntries(pendingRows.value.map((row) => {
    const targets = row.pushTargets?.filter(target => target.available) ?? []
    return [row.id, targets.length === 1 ? targets[0]!.accountId : '']
  }))
  confirming.value = pendingRows.value.length > 0
  if (mode === 'push' && pendingRows.value.some(row => canPush(row) && row.poolAccountIds.length === 0))
    void loadPushGroups()
}
function canPush(row: ReloginEntry) {
  return !maintenanceBusy(row) && !row.maintenance?.uncertain && row.status === 'ready' && row.credentialStatus === 'verified' && row.poolStatus !== 'synced'
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
    label: `${reloginPlanDisplay(target, '未知套餐')} · ${target.workspaceId}`,
  }))
}
const pushSelectionMissing = computed(() => pushable.value.some(row =>
  requiresPushTarget(row) && !pushTarget(row),
))
async function executeConfirmed() {
  if (confirmMode.value === 'push' && pushSelectionMissing.value)
    return
  if (confirmMode.value === 'push' && newPushCount.value > 0) {
    if (pushProxyError.value || await pushFields.value?.validate() === false)
      return
  }
  return action(async () => {
    if (confirmMode.value === 'push') {
      const template = newPushCount.value > 0 ? accountImportTemplate(pushForm.value, 'openai') : undefined
      const selections: Record<string, ReloginPushSelection> = {}
      for (const row of pushable.value) {
        const target = pushTarget(row)
        if (requiresPushTarget(row) && target)
          selections[row.id] = { accountId: target.accountId, switchWorkspace: target.switchWorkspace }
      }
      const newAccount = newPushCount.value > 0
        ? {
            settings: accountImportSettings(pushForm.value, 'openai'),
            outboundProxyId: accountCreateEgress(pushForm.value, true).outboundProxyId || undefined,
            qualityTemplate: accountImportQualityTemplate(pushForm.value),
          }
        : undefined
      batchReport(await pushRelogin(pushable.value, template, undefined, Object.keys(selections).length ? selections : undefined, undefined, newAccount))
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
watch([search, plan, status, pool, automatic, recent, webSessionFilter, egressFilter, pageSize], () => {
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
  <div class="flex min-w-0 flex-col gap-3 md:h-full md:min-h-0 md:gap-4 md:overflow-hidden">
    <BasePageHeader class="relogin-page-header" :title="t('navigation.relogin')" :description="mobile ? undefined : '账号资料 · 登录会话 · 安全维护'">
      <template #actions>
        <BaseIconButton label="刷新重登列表" :loading="loading" :disabled="busy" @click="reload()">
          <RefreshCw class="size-4" />
        </BaseIconButton>

        <BaseIconButton label="维护设置" :disabled="busy" @click="openRetrySettings">
          <Settings2 class="size-4" />
        </BaseIconButton>
        <BaseButton v-if="enrollmentPreview" variant="primary" :disabled="busy" @click="action(() => previewPlanCommand({ mode: 'sample' }))">
          添加示例资料
        </BaseButton>
        <BaseButton v-else variant="primary" :disabled="busy" @click="importing = true">
          <template #icon>
            <Upload class="size-4" />
          </template>导入
        </BaseButton>
      </template>
    </BasePageHeader>
    <div class="relogin-summary" aria-label="账号维护概览">
      <button v-for="item in summary" :key="item.key" type="button" class="relogin-summary-item" :class="{ active: status === item.key }" @click="status = item.key">
        <span>{{ item.label }}</span><strong>{{ item.count }}</strong>
      </button>
      <div class="relogin-queue-meta">
        <span :class="paused ? 'text-cp-warning' : 'text-cp-success'">{{ paused ? '队列已暂停' : '队列运行中' }}</span><span class="hidden md:inline">{{ savedConcurrency }} 个任务并行 · 失败重试 {{ savedRetrySettings.maxRetries }} 次</span>
      </div>
    </div>
    <div class="flex items-center gap-2 md:flex-wrap md:gap-3">
      <BaseInput v-model="search" :class="mobile ? 'min-w-0 flex-1' : 'w-full sm:max-w-80'" aria-label="搜索账号名称或邮箱" placeholder="搜索账号名称或邮箱" clearable>
        <template #prefix>
          <Search class="size-4" />
        </template>
      </BaseInput>
      <BaseButton v-if="mobile" class="min-h-11 whitespace-nowrap px-3" :variant="filterCount ? 'soft' : 'secondary'" @click="mobileFiltersOpen = true">
        <template #icon>
          <Funnel class="size-4" />
        </template>筛选{{ filterCount ? ` ${filterCount}` : '' }}
      </BaseButton>
      <span v-if="!mobile" class="ml-auto text-cp-xs tabular-nums text-cp-text-tertiary">{{ filtered.length }} / {{ entries.length }} 个账号</span>
      <BaseButton v-if="!mobile" :disabled="!search && !filterCount" @click="search = ''; status = ''; plan = ''; pool = ''; automatic = ''; recent = ''; webSessionFilter = ''; egressFilter = ''">
        清除筛选{{ filterCount ? `（${filterCount}）` : '' }}
      </BaseButton>
    </div>
    <div class="flex shrink-0 flex-wrap items-center gap-2 border-y border-cp-border py-1 md:py-2">
      <BaseCheckbox class="min-h-11 min-w-8 justify-center md:hidden" label="全选当前页" :model-value="allSelected" :indeterminate="partialSelected" :disabled="busy" @update:model-value="selectPage" />
      <span class="text-cp-sm text-cp-text-secondary">已选 {{ selected.size }} 项</span>
      <span v-if="mobile" class="ml-auto text-cp-xs tabular-nums text-cp-text-tertiary">{{ filtered.length }} / {{ entries.length }} 个账号</span>
      <BaseButton v-if="mobile && (search || filterCount)" class="min-h-11 px-2" variant="ghost" @click="search = ''; status = ''; plan = ''; pool = ''; automatic = ''; recent = ''; webSessionFilter = ''; egressFilter = ''">
        清除筛选
      </BaseButton>
      <div v-if="selected.size" class="flex flex-wrap items-center gap-2">
        <BaseIconButton :label="t('settings.pricing.deselect')" :disabled="busy || !selected.size" @click="selected = new Set()">
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
        <BaseButton :disabled="busy || !selected.size || selected.size > 200" @click="copySelected">
          复制 2FA
        </BaseButton>
        <BaseButton :disabled="busy || !selected.size || selected.size > 200" @click="confirmExport('json')">
          <template #icon>
            <Download class="size-4" />
          </template>导出JSON
        </BaseButton>
        <BaseButton :disabled="busy || !selected.size || selected.size > 200" @click="confirmExport('two_fa')">
          <template #icon>
            <Download class="size-4" />
          </template>导出2FA
        </BaseButton>
        <BaseIconButton label="批量关闭自动重登" :disabled="busy || !selected.size || selected.size > 500" @click="changeAutomatic([...selected], false)">
          <Pause class="size-4" />
        </BaseIconButton>
        <BaseIconButton label="批量删除资料" :disabled="busy || !selected.size || selected.size > 500" @click="confirm('delete', [...selected])">
          <Trash2 class="size-4 text-cp-error" />
        </BaseIconButton>
      </div>
    </div>
    <div v-if="failure || loadFailure" class="max-h-24 shrink-0 overflow-auto whitespace-pre-wrap break-words text-cp-sm text-cp-error" role="alert">
      {{ failure || loadFailure }}
    </div>
    <ReloginMaintenanceModal v-model="maintenanceOpen" :entry="maintenanceEntry" :initial-tab="maintenanceTab" @changed="reload(true)" @credentials="credentialsEntry = maintenanceEntry; credentialsOpen = true" />
    <ReloginHistoryModal v-model="historyOpen" :entry="historyEntry" @after-leave="!historyOpen && (historyEntry = null)" />
    <EnrollmentPlanModal v-if="arranging" :key="arranging.id" :row="arranging" :paused="paused" @close="arranging = null" @changed="reload(true)" />
    <ReloginCredentialsModal v-model="credentialsOpen" :entry="credentialsEntry" @changed="reload(true)" @after-leave="!credentialsOpen && (credentialsEntry = null)" />
    <section v-if="!mobile" class="flex min-h-48 min-w-0 flex-1 flex-col overflow-hidden rounded-cp-card border border-cp-border-secondary bg-cp-bg-container">
      <BaseTable ref="table" class="relogin-table min-h-0 flex-1" :columns="columns" :rows="visible" :selected-row-keys="[...selected]" :loading="loading && !entries.length" column-layout="proportional" :empty-text="t('appearance.preview.components.noMatchingAccountYet')" @mousedown="onMouseDown">
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
              <button type="button" class="block max-w-full truncate text-left font-semibold text-cp-link hover:underline disabled:text-cp-text disabled:no-underline" title="查看 2FA / JSON" @mousedown.stop @click.stop="credentialsEntry = row; credentialsOpen = true">
                {{ reloginDisplayName(row) }}
              </button>
              <div v-if="reloginDisplayName(row) !== row.email" class="truncate text-cp-xs text-cp-text-secondary" :title="row.email">
                {{ row.email }}
              </div>
            </div>
          </div>
        </template>
        <template #header-plan>
          <ReloginColumnFilter label="PLAN" :active="!!plan">
            <ReloginFilterOptions v-model="plan" :options="plans" label="PLAN 筛选" />
          </ReloginColumnFilter>
        </template>
        <template #header-credential>
          <ReloginColumnFilter label="登录状态" :active="!!webSessionFilter || !!pool">
            <ReloginFilterOptions v-model="webSessionFilter" :options="sessionOptions" label="Web 会话筛选" /><ReloginFilterOptions v-model="pool" :options="poolOptions" label="号池状态筛选" />
          </ReloginColumnFilter>
        </template>
        <template #header-egress>
          <ReloginColumnFilter label="出口 IP" :active="!!egressFilter">
            <ReloginFilterOptions v-model="egressFilter" :options="egressOptions" label="出口筛选" />
          </ReloginColumnFilter>
        </template>
        <template #header-status>
          <ReloginColumnFilter label="最近操作" :active="!!status || !!recent">
            <ReloginFilterOptions v-model="status" :options="statusOptions" label="处理状态筛选" /><ReloginFilterOptions v-model="recent" :options="recentOptions" label="最近操作时间筛选" />
          </ReloginColumnFilter>
        </template>
        <template #header-automatic>
          <ReloginColumnFilter label="自动恢复" :active="!!automatic">
            <ReloginFilterOptions v-model="automatic" :options="autoOptions" label="自动恢复筛选" />
          </ReloginColumnFilter>
        </template>
        <template #plan="{ row }">
          <AccountPlanBadge :plan-type="row.planType" :plan-type-display="reloginPlanDisplay(row)" size="sm" />
          <div class="mt-1 whitespace-normal break-words text-cp-xs text-cp-text-secondary [overflow-wrap:anywhere]" :title="workspaceId(row) ?? '自动选择工作区'">
            {{ workspaceLabel(row) }}
          </div>
        </template>
        <template #egress="{ row }">
          <ReloginEgress :row="row" />
        </template>
        <template #status="{ row }">
          <EnrollmentPlanStatus v-if="row.enrollmentPlan" :row="row" />
          <template v-if="!row.enrollmentPlan || !['draft', 'scheduled'].includes(row.enrollmentPlan.state)">
            <button type="button" class="mb-1 block text-left font-medium text-cp-link hover:underline" @click="historyEntry = row; historyOpen = true">
              {{ recentOperation(row) }}
            </button>
            <button v-if="row.status === 'awaiting_workspace'" type="button" class="block cursor-pointer border-0 bg-transparent p-0 text-left text-cp-sm text-cp-warning underline underline-offset-4 disabled:cursor-not-allowed disabled:opacity-50" :disabled="busy || paused" @click="edit(row)">
              待选择工作区
            </button>
            <span v-else class="block whitespace-normal break-words" :class="maintenanceResult(row).tone" :title="row.maintenance?.message || processingStatus(row).detail">{{ maintenanceResult(row).label }}</span>
            <button v-if="recentOperationAt(row)" type="button" class="mt-1 block whitespace-nowrap text-cp-xs tabular-nums text-cp-link hover:underline" title="查看操作记录" @click="historyEntry = row; historyOpen = true">
              {{ formatDateTime(recentOperationAt(row)) }}
            </button>
            <span v-if="retryProgress(row)" class="block text-cp-xs tabular-nums text-cp-text-tertiary">{{ retryProgress(row) }}</span>
            <span v-if="row.recovery?.retryAt" class="block whitespace-normal break-words text-cp-xs tabular-nums text-cp-text-tertiary">{{ recoveryCountdown(row, now.getTime()) }}</span>
          </template>
        </template>
        <template #credential="{ row }">
          <ReloginSessionStatus :row="row" />
        </template>
        <template #automatic="{ row }">
          <BaseSwitch :label="`${row.email} 自动重登`" :model-value="row.automatic" :disabled="busy" @update:model-value="changeAutomatic([row.id], $event)" />
          <div class="mt-1 text-cp-xs text-cp-text-tertiary">
            成功 <ReloginCountCell :count="row.reloginCount" :last-relogin-at="row.lastReloginAt" /> 次
          </div>
        </template>
        <template #actions="{ row }">
          <div class="flex flex-nowrap items-center gap-1 whitespace-nowrap">
            <BaseButton v-if="canArrange(row)" size="sm" variant="soft" :disabled="busy || ['queued', 'running', 'pushing', 'uncertain'].includes(row.status)" @click="arranging = row">
              {{ row.enrollmentPlan ? '查看安排' : '安排入池' }}
            </BaseButton>
            <BaseButton v-else-if="canPush(row)" size="sm" variant="soft" :disabled="busy" @click="confirm('push', [row.id])">
              推送凭据
            </BaseButton>
            <BaseButton v-else size="sm" :title="queueDisabledReason(row, busy, paused)" :disabled="!!queueDisabledReason(row, busy, paused)" @click="queue([row.id])">
              {{ queueButtonLabel(row, paused) }}
            </BaseButton>
            <BasePopover trigger="click" placement="bottom-end">
              <template #trigger>
                <BaseIconButton label="更多账号操作" size="sm">
                  <MoreHorizontal class="size-4" />
                </BaseIconButton>
              </template>
              <template #default="{ close }">
                <div class="min-w-40 p-1">
                  <BaseMenuItem v-if="canArrange(row) && canPush(row)" :disabled="busy || row.enrollmentPlan?.state === 'scheduled'" @click="close(); confirm('push', [row.id])">
                    推送已有凭据
                  </BaseMenuItem>
                  <BaseMenuItem v-if="canArrange(row)" :disabled="!!queueDisabledReason(row, busy, paused) || row.enrollmentPlan?.state === 'scheduled'" @click="close(); queue([row.id])">
                    仅重登，不推送
                  </BaseMenuItem>
                  <BaseMenuItem @click="close(); openMaintenance(row)">
                    登录与授权
                  </BaseMenuItem>
                  <BaseMenuItem @click="close(); openMaintenance(row, 'devices')">
                    设备与会话
                  </BaseMenuItem>
                  <BaseMenuItem @click="close(); openMaintenance(row, 'workspaces')">
                    工作区
                  </BaseMenuItem>
                  <BaseMenuItem @click="close(); credentialsEntry = row; credentialsOpen = true">
                    账号资料与复制
                  </BaseMenuItem>
                  <BaseMenuItem @click="close(); openMaintenance(row, 'security')">
                    安全维护
                  </BaseMenuItem>
                  <BaseMenuItem @click="close(); historyEntry = row; historyOpen = true">
                    操作历史
                  </BaseMenuItem>
                  <BaseMenuItem :disabled="maintenanceBusy(row)" @click="close(); edit(row)">
                    自动恢复工作区
                  </BaseMenuItem>
                  <BaseMenuItem v-if="maintenanceBusy(row)" @click="close(); cancelTask(row)">
                    取消任务
                  </BaseMenuItem>
                  <BaseMenuItem tone="destructive" @click="close(); confirm('delete', [row.id])">
                    删除本地资料
                  </BaseMenuItem>
                </div>
              </template>
            </BasePopover>
          </div>
        </template>
      </BaseTable>
    </section>
    <section v-else class="grid gap-3" aria-label="重登账号卡片">
      <p v-if="loading && !entries.length" role="status" class="text-cp-text-secondary">
        正在读取账号
      </p>
      <p v-else-if="!visible.length" class="text-cp-text-secondary">
        {{ t('appearance.preview.components.noMatchingAccountYet') }}
      </p>
      <ReloginMobileCard
        v-for="row in visible" :key="row.id" :row="row" :selected="selected.has(row.id)" :busy="busy" :paused="paused" :can-push="canPush(row)" :now="now.getTime()"
        @select="toggle(row.id, $event)" @automatic="changeAutomatic([row.id], $event)"
        @arrange="arranging = row"
        @credentials="credentialsEntry = row; credentialsOpen = true" @history="historyEntry = row; historyOpen = true"
        @cancel="cancelTask(row)" @queue="queue([row.id])" @maintain="openMaintenance(row, $event)" @push="confirm('push', [row.id])" @workspace="edit(row)" @delete="confirm('delete', [row.id])"
      />
    </section>
    <BaseTablePagination :pagination="{ currentPage: page, pageSize, total: filtered.length }" :loading="busy" @page-change="page = $event" @page-size-change="pageSize = $event" />
    <div v-if="overlayStyle" :style="overlayStyle" class="pointer-events-none fixed z-50 border border-cp-primary bg-cp-primary/10" />

    <BaseModal v-model="mobileFiltersOpen" title="筛选账号" size="sm">
      <div class="grid gap-4">
        <BaseFormItem label="PLAN">
          <BaseSelect v-model="plan" :options="plans" aria-label="PLAN 筛选" />
        </BaseFormItem>
        <BaseFormItem label="Web 会话">
          <BaseSelect v-model="webSessionFilter" :options="sessionOptions" aria-label="Web 会话筛选" />
        </BaseFormItem>
        <BaseFormItem label="号池状态">
          <BaseSelect v-model="pool" :options="poolOptions" aria-label="号池状态筛选" />
        </BaseFormItem>
        <BaseFormItem label="出口 IP">
          <BaseSelect v-model="egressFilter" :options="egressOptions" aria-label="出口筛选" />
        </BaseFormItem>
        <BaseFormItem label="处理状态">
          <BaseSelect v-model="status" :options="statusOptions" aria-label="处理状态筛选" />
        </BaseFormItem>
        <BaseFormItem label="最近操作">
          <BaseSelect v-model="recent" :options="recentOptions" aria-label="最近操作时间筛选" />
        </BaseFormItem>
        <BaseFormItem label="自动恢复">
          <BaseSelect v-model="automatic" :options="autoOptions" aria-label="自动恢复筛选" />
        </BaseFormItem>
      </div>
      <template #footer>
        <BaseButton class="min-h-11" :disabled="!filterCount" @click="status = ''; plan = ''; pool = ''; automatic = ''; recent = ''; webSessionFilter = ''; egressFilter = ''">
          重置筛选
        </BaseButton>
        <BaseButton class="min-h-11" variant="primary" @click="mobileFiltersOpen = false">
          查看 {{ filtered.length }} 个账号
        </BaseButton>
      </template>
    </BaseModal>
    <BaseModal v-model="retrySettingsOpen" title="维护设置" size="sm" :dismissible="!busy">
      <div class="grid min-w-0 gap-4">
        <div class="flex flex-wrap items-center gap-2 border-b border-cp-border-secondary pb-4">
          <span class="text-cp-sm text-cp-text-secondary">{{ t('groups.metrics.concurrency') }}</span>
          <BaseInput v-model="concurrency" class="w-18" type="number" min="1" max="8" step="1" aria-label="重登并发" :disabled="busy" />
          <BaseIconButton label="保存并发设置" :disabled="busy || concurrency === String(savedConcurrency)" @click="configure()">
            <Save class="size-4" />
          </BaseIconButton>
          <BaseIconButton :label="paused ? '恢复重登队列' : '暂停重登队列'" :disabled="busy" @click="configure(!paused)">
            <Play v-if="paused" class="size-4 text-cp-success" /><Pause v-else class="size-4" />
          </BaseIconButton>
        </div>
        <BaseFormItem label="失败重试次数（不含首次，0 为不重试）">
          <BaseInput v-model="maxRetries" type="number" min="0" max="10" step="1" aria-label="失败重试次数" :disabled="busy" />
        </BaseFormItem>
        <BaseFormItem label="重试间隔（秒）">
          <BaseInput v-model="retryIntervalSeconds" type="number" min="1" max="86400" step="1" aria-label="重试间隔（秒）" :disabled="busy" />
        </BaseFormItem>
        <div v-if="retrySettingsError" class="break-words text-cp-sm text-cp-error" role="alert">
          {{ retrySettingsError }}
        </div>
      </div>
      <template #footer>
        <BaseButton :disabled="busy" @click="retrySettingsOpen = false">
          {{ t('common.actions.cancel') }}
        </BaseButton>
        <BaseButton variant="primary" :loading="busy" @click="saveRetrySettings">
          {{ t('common.actions.save') }}
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
        <BaseTextarea v-model="importText" aria-label="账号资料" autocomplete="off" autocapitalize="off" wrap="off" class="[&_textarea]:overflow-x-auto [&_textarea]:whitespace-pre" :spellcheck="false" :rows="7" :disabled="busy" />
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
    <BaseConfirmModal v-model="exportOpen" :title="exportFormat === 'json' ? '导出JSON' : '导出2FA'" :loading="busy" @confirm="executeExport">
      <p class="m-0 text-cp-sm">
        将导出所选 {{ exportIds.length }} 条资料，文件包含{{ exportFormat === 'json' ? '访问凭据' : '邮箱、密码和2FA密钥' }}，请妥善保管。
      </p>
    </BaseConfirmModal>
    <BaseModal v-model="queueOpen" title="立即重登" :description="queueDescription" size="sm" :dismissible="!busy">
      <div class="grid gap-4">
        <p v-if="failure" role="alert" class="m-0 whitespace-pre-wrap break-words text-cp-sm text-cp-error">
          {{ failure }}
        </p>
        <div role="radiogroup" aria-label="本次登录工作区" class="grid gap-2">
          <button v-for="option in queueOptions" :key="option.value" type="button" role="radio" :aria-checked="queueMode === option.value" :disabled="busy" class="min-w-0 rounded-cp-lg focus-visible:outline-2 focus-visible:outline-cp-primary" @click="queueMode = option.value">
            <ReloginWorkspaceCard :label="option.label" :description="option.description" :selected="queueMode === option.value" />
          </button>
          <div v-if="queueWorkspaces.length" class="mt-2 text-cp-xs text-cp-text-tertiary">
            已读取的工作区
          </div>
          <button v-for="choice in queueWorkspaces" :key="choice.id" type="button" role="radio" :aria-checked="queueMode === `workspace:${choice.id}`" :disabled="busy || !choice.available" class="min-w-0 rounded-cp-lg focus-visible:outline-2 focus-visible:outline-cp-primary disabled:opacity-50" @click="queueMode = `workspace:${choice.id}`">
            <ReloginWorkspaceCard :workspace="choice" :selected="queueMode === `workspace:${choice.id}`" />
          </button>
        </div>
        <p class="m-0 text-cp-sm text-cp-text-secondary">
          完成后待你确认推送
        </p>
      </div>
      <template #footer>
        <BaseButton variant="secondary" :disabled="busy" @click="queueOpen = false">
          取消
        </BaseButton>
        <BaseButton variant="primary" :loading="busy" @click="executeQueue">
          加入队列
        </BaseButton>
      </template>
    </BaseModal>
    <BaseModal
      v-model="confirming"
      :title="confirmMode === 'push' ? '推送凭据' : '删除重登资料'"
      :size="confirmMode === 'push' ? 'lg' : 'sm'"
      :tone="confirmMode === 'delete' ? 'danger' : 'neutral'"
      :role="confirmMode === 'delete' ? 'alertdialog' : 'dialog'"
      :dismissible="!busy"
    >
      <div class="grid min-w-0 gap-5 text-cp-sm">
        <p v-if="failure" class="m-0 whitespace-pre-wrap break-words text-cp-error" role="alert">
          {{ failure }}
        </p>
        <p v-if="confirmMode === 'delete'" class="m-0 text-cp-text-secondary">
          删除 {{ pendingRows.length }} 项资料并取消相关重登任务，号池账号不受影响。
        </p>
        <div v-else class="flex flex-wrap items-center gap-x-5 gap-y-2 text-cp-text-secondary">
          <span v-if="newPushCount">新增入池 <strong class="ml-1 text-cp-text">{{ newPushCount }}</strong></span>
          <span v-if="pushable.length > newPushCount">更新凭据 <strong class="ml-1 text-cp-text">{{ pushable.length - newPushCount }}</strong></span>
          <span v-if="pendingRows.length > pushable.length">跳过 <strong class="ml-1 text-cp-warning">{{ pendingRows.length - pushable.length }}</strong></span>
        </div>
        <div class="grid min-w-0 gap-2">
          <div v-for="row in pendingRows" :key="row.id" class="min-w-0 rounded-cp border border-cp-border-secondary p-3 sm:p-4">
            <div class="flex min-w-0 flex-wrap items-start justify-between gap-3">
              <div class="min-w-0 flex-1">
                <div class="break-all font-semibold text-cp-text">
                  {{ reloginDisplayName(row) }}
                </div>
                <div v-if="reloginDisplayName(row) !== row.email" class="mt-1 break-all text-cp-xs text-cp-text-secondary">
                  {{ row.email }}
                </div>
              </div>
              <div v-if="confirmMode === 'push'" class="flex flex-wrap items-center gap-2">
                <AccountPlanBadge :plan-type="row.planType" :plan-type-display="reloginPlanDisplay(row)" size="sm" />
                <span class="text-cp-xs" :class="canPush(row) ? 'text-cp-text-secondary' : 'text-cp-warning'">
                  {{ !canPush(row) ? '不可推送' : row.poolAccountIds.length ? '更新凭据' : '新增入池' }}
                </span>
              </div>
            </div>
            <div v-if="confirmMode === 'push' && canPush(row) && requiresPushTarget(row)" class="mt-3 grid min-w-0 gap-2">
              <BaseFormItem label="更新目标">
                <BaseSelect v-model="pushAccounts[row.id]" :options="pushTargetOptions(row)" :aria-label="`${row.email} 推送目标`" placeholder="选择要更新的账号" :disabled="busy" />
              </BaseFormItem>
              <p v-if="pushTarget(row)?.switchWorkspace" class="m-0 break-words text-cp-warning">
                将原 {{ reloginPlanDisplay(pushTarget(row), '未知套餐') }} 账号切换为 {{ reloginPlanDisplay(row) }}，不另建账号。
              </p>
              <p v-else-if="!pushTarget(row)" class="m-0 text-cp-warning">
                {{ pushTargetOptions(row).length ? '请选择推送目标。' : '原账号已变化或目标工作区已存在，请重新获取凭据。' }}
              </p>
            </div>
            <details v-if="row.workspaceId && confirmMode === 'push'" class="mt-3 text-cp-xs text-cp-text-secondary">
              <summary class="cursor-pointer">
                工作区：{{ row.maintenance?.workspaces.find(item => item.id === row.workspaceId)?.name || row.workspaceChoices?.find(item => item.id === row.workspaceId)?.name || shortWorkspace(row.workspaceId) }}
              </summary>
              <div class="mt-2 break-all font-mono">
                {{ row.workspaceId }}
              </div>
            </details>
          </div>
        </div>
        <section v-if="confirmMode === 'push' && newPushCount > 0" class="grid min-w-0 gap-4 border-t border-cp-border-secondary pt-4" aria-label="新账号设置">
          <h3 class="m-0 text-cp-sm font-semibold">
            新账号设置
          </h3>
          <AccountSetupFields
            v-if="confirming"
            ref="pushFields"
            v-model="pushForm"
            hide-provider
            :groups="pushGroups"
            :groups-loading="pushGroupsLoading"
            :disabled="busy"
            :proxy-error="pushProxyError"
          />
        </section>
      </div>
      <template #footer>
        <BaseButton variant="secondary" :disabled="busy" @click="confirming = false">
          {{ t('ui.cancel') }}
        </BaseButton>
        <BaseButton :variant="confirmMode === 'delete' ? 'destructive' : 'primary'" :loading="busy" :disabled="confirmMode === 'push' && (!pushable.length || pushSelectionMissing)" @click="executeConfirmed">
          {{ confirmMode === 'push' ? '推送凭据' : '删除资料' }}
        </BaseButton>
      </template>
    </BaseModal>
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
.relogin-summary {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}
.relogin-summary-item {
  display: flex;
  align-items: center;
  gap: 16px;
  border: 1px solid var(--cp-border-secondary);
  border-radius: 8px;
  background: var(--cp-card-bg);
  padding: 12px 18px;
  color: var(--cp-text-secondary);
  cursor: pointer;
}
.relogin-summary-item strong {
  font-size: 22px;
  line-height: 1;
  font-variant-numeric: tabular-nums;
  color: var(--cp-text);
}
.relogin-summary-item.active {
  border-color: var(--cp-primary);
  background: var(--cp-primary-bg);
}
.relogin-queue-meta {
  margin-left: auto;
  display: grid;
  gap: 4px;
  font-size: 12px;
  color: var(--cp-text-tertiary);
}
@media (max-width: 767px) {
  .relogin-page-header {
    gap: 8px;
    padding-bottom: 0;
  }
  .relogin-page-header :deep(> div:first-child) {
    flex-basis: auto;
  }
  .relogin-page-header :deep(h1) {
    font-size: 20px;
  }
  .relogin-page-header :deep(> div:last-child) {
    gap: 4px;
    margin-top: 0;
  }
  .relogin-page-header :deep(button) {
    min-height: 44px;
    min-width: 44px;
  }
  .relogin-summary {
    display: grid;
    grid-template-columns: repeat(5, minmax(0, 1fr));
    gap: 4px;
  }
  .relogin-summary-item {
    flex-direction: column;
    gap: 6px;
    padding: 8px 4px;
    font-size: 11px;
  }
  .relogin-summary-item strong {
    font-size: 18px;
  }
  .relogin-queue-meta {
    grid-column: 1 / -1;
    margin: 4px 0 0;
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
  }
}

.relogin-table :deep([data-column-key='plan']),
.relogin-table :deep([data-column-key='credential']) {
  padding-left: 8px;
  padding-right: 8px;
}
.relogin-table :deep([data-column-key='identity']),
.relogin-table :deep([data-column-key='status']) {
  padding-left: 12px;
  padding-right: 12px;
}
@media (max-width: 1599px) {
  .relogin-table :deep(th[data-column-key='actions']) {
    right: auto !important;
  }
  .relogin-table :deep(td[data-column-key='actions']) {
    position: static;
  }
}
</style>
