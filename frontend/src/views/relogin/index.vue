<script setup lang="ts">
import type { ReloginBatchResult, ReloginEntry, ReloginStatus } from '@/api/modules/relogin'
import { CheckCheck, GripVertical, Pause, Play, RefreshCw, Save, Search, Settings2, Trash2, Upload, X } from '@lucide/vue'
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import {
  configureRelogin,
  deleteRelogin,
  getRelogin,
  importRelogin,
  pushRelogin,
  queueRelogin,
  setReloginAutomatic,
  setReloginWorkspace,
} from '@/api/modules/relogin'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
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
import ReloginCountCell from '@/components/ReloginCountCell.vue'
import { errorMessage } from '@/utils/async'
import { formatDateTime } from '@/utils/date'
import { useAccountSwipeSelect } from '../accounts/composables/useAccountSwipeSelect'
import { importPreview } from './import-preview'

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
const statuses: Record<ReloginStatus, string> = {
  pending: '待处理',
  queued: '排队中',
  running: '重登中',
  ready: '已获取新 JSON',
  pushing: '推送中',
  uncertain: '推送待核实',
  failed: '失败',
}
const poolLabels = { absent: '未在池中', present: '已在池中', pending_push: '新凭证待推送', synced: '已同步' }
const credentialLabels = { none: '无凭证', verified: '验证通过', expired: '已过期' }
const statusOptions = [{ value: '', label: '全部处理状态' }, ...Object.entries(statuses).map(([value, label]) => ({ value, label }))]
const poolOptions = [{ value: '', label: '全部入池状态' }, ...Object.entries(poolLabels).map(([value, label]) => ({ value, label }))]
const autoOptions = [{ value: '', label: '全部自动重登' }, { value: 'on', label: '允许自动重登' }, { value: 'off', label: '关闭自动重登' }]
const plans = computed(() => [{ value: '', label: '全部 PLAN' }, ...[...new Set(entries.value.map(row => row.planType).filter((value): value is string => !!value))].sort().map(value => ({ value, label: value.toUpperCase() }))])
const filtered = computed(() => entries.value.filter(row =>
  row.email.toLowerCase().includes(search.value.trim().toLowerCase())
  && (!plan.value || row.planType === plan.value)
  && (!status.value || row.status === status.value)
  && (!pool.value || row.poolStatus === pool.value)
  && (!automatic.value || row.automatic === (automatic.value === 'on')),
))
const visible = computed(() => filtered.value.slice((page.value - 1) * pageSize.value, page.value * pageSize.value))
const allSelected = computed(() => visible.value.length > 0 && visible.value.every(row => selected.value.has(row.id)))
const partialSelected = computed(() => !allSelected.value && visible.value.some(row => selected.value.has(row.id)))
const columns = defineTableColumns<ReloginEntry>([
  { key: 'selection', kind: 'selection' },
  { key: 'identity', label: '账号', kind: 'identity', size: '2xl', grow: 1 },
  { key: 'plan', label: 'PLAN / 工作区', kind: 'custom', size: 'lg' },
  { key: 'status', label: '处理状态', kind: 'status', size: 'lg' },
  { key: 'credential', label: '凭证', kind: 'status' },
  { key: 'pool', label: '号池', kind: 'status', size: 'lg' },
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
function queue(ids: string[]) {
  return action(async () => batchReport(await queueRelogin(ids)))
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
    toast.success(`已导入 ${result.imported} 个账号`)
  })
}
const confirming = shallowRef(false)
const confirmMode = shallowRef<'push' | 'delete'>('push')
const pendingRows = shallowRef<ReloginEntry[]>([])
function confirm(mode: 'push' | 'delete', ids: string[]) {
  confirmMode.value = mode
  pendingRows.value = entries.value.filter(row => ids.includes(row.id)).map(row => ({ ...row }))
  confirming.value = pendingRows.value.length > 0
}
function canPush(row: ReloginEntry) {
  return row.status === 'ready' && row.credentialStatus === 'verified' && row.poolStatus !== 'synced'
}
const pushable = computed(() => pendingRows.value.filter(canPush))
function executeConfirmed() {
  return action(async () => {
    if (confirmMode.value === 'push') {
      batchReport(await pushRelogin(pushable.value))
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
function edit(row: ReloginEntry) {
  editRow.value = row
  workspace.value = row.preferredWorkspaceId ?? ''
  editing.value = true
}
function saveWorkspace() {
  const row = editRow.value
  if (!row)
    return
  return action(async () => {
    await setReloginWorkspace(row.id, workspace.value.trim() || null)
    editing.value = false
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
        <BaseButton variant="primary" :disabled="busy" @click="importing = true">
          <template #icon>
            <Upload class="size-4" />
          </template>导入
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
      <BaseSelect v-model="pool" :options="poolOptions" aria-label="入池状态筛选" />
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
      <BaseTable ref="table" class="min-h-0 flex-1" :columns="columns" :rows="visible" :selected-row-keys="[...selected]" :loading="loading && !entries.length" column-layout="content" empty-text="暂无匹配账号" @mousedown="onMouseDown">
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
            </div>
          </div>
        </template>
        <template #plan="{ row }">
          <div class="font-mono text-cp-sm">
            {{ row.planType?.toUpperCase() ?? '未知' }}
          </div>
          <div class="truncate text-cp-xs text-cp-text-tertiary" :title="row.workspaceId ?? row.preferredWorkspaceId ?? ''">
            {{ row.workspaceId ?? row.preferredWorkspaceId ?? '自动选择' }}
          </div>
        </template>
        <template #status="{ row }">
          <span :class="row.status === 'failed' ? 'text-cp-error' : row.status === 'running' ? 'text-cp-primary' : ''">{{ statuses[row.status] }}</span>
        </template>
        <template #credential="{ row }">
          <span :class="row.credentialStatus === 'verified' ? 'text-cp-success' : row.credentialStatus === 'expired' ? 'text-cp-warning' : 'text-cp-text-tertiary'" :title="row.verifiedAt ? `验证于 ${formatDateTime(row.verifiedAt)}` : ''">{{ credentialLabels[row.credentialStatus] }}</span>
        </template>
        <template #pool="{ row }">
          <span :class="row.poolStatus === 'synced' ? 'text-cp-success' : 'text-cp-text-secondary'">{{ poolLabels[row.poolStatus] }}</span>
        </template>
        <template #automatic="{ row }">
          <BaseSwitch :label="`${row.email} 自动重登`" :model-value="row.automatic" :disabled="busy" @update:model-value="changeAutomatic([row.id], $event)" />
        </template>
        <template #reloginCount="{ row }">
          <ReloginCountCell :count="row.reloginCount" :last-relogin-at="row.lastReloginAt" />
        </template>
        <template #actions="{ row }">
          <div class="flex items-center gap-1">
            <BaseIconButton label="重登获取凭证" size="sm" :disabled="busy || ['queued', 'running', 'pushing'].includes(row.status)" :loading="row.status === 'running'" @click="queue([row.id])">
              <RefreshCw class="size-3.5" />
            </BaseIconButton>
            <BaseIconButton label="推送到号池" size="sm" :disabled="busy || !canPush(row)" @click="confirm('push', [row.id])">
              <Upload class="size-3.5 text-cp-link" />
            </BaseIconButton>
            <BaseIconButton label="工作区设置" size="sm" :disabled="busy" @click="edit(row)">
              <Settings2 class="size-3.5" />
            </BaseIconButton>
            <BaseIconButton label="删除重登资料" size="sm" :disabled="busy" @click="confirm('delete', [row.id])">
              <Trash2 class="size-3.5 text-cp-error" />
            </BaseIconButton>
          </div>
        </template>
      </BaseTable>
    </section>
    <BaseTablePagination :pagination="{ currentPage: page, pageSize, total: filtered.length }" :loading="busy" @page-change="page = $event" @page-size-change="pageSize = $event" />
    <div v-if="overlayStyle" :style="overlayStyle" class="pointer-events-none fixed z-50 border border-cp-primary bg-cp-primary/10" />

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
    <BaseConfirmModal v-model="confirming" :title="confirmMode === 'push' ? '确认推送到号池' : '删除重登资料'" :destructive="confirmMode === 'delete'" :loading="busy" :confirm-disabled="confirmMode === 'push' && !pushable.length" @confirm="executeConfirmed">
      <p v-if="confirmMode === 'delete'" class="mt-0 text-cp-sm">
        删除 {{ pendingRows.length }} 项资料并取消相关重登任务，号池账号不受影响。
      </p>
      <p v-else class="mt-0 text-cp-sm">
        推送 {{ pushable.length }} 项，跳过 {{ pendingRows.length - pushable.length }} 项。
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
        </div>
      </div>
    </BaseConfirmModal>
    <BaseModal v-model="editing" title="工作区设置" :dismissible="!busy">
      <div class="grid gap-3">
        <p class="m-0 break-all text-cp-sm">
          {{ editRow?.email }}
        </p>
        <BaseInput v-model="workspace" aria-label="指定工作区 ID" placeholder="自动选择优先套餐" :disabled="busy" />
        <p class="m-0 text-cp-sm text-cp-warning">
          修改后需重新获取凭证。已在池中的账号恢复时锁定原工作区。
        </p>
      </div>
      <template #footer>
        <BaseButton variant="primary" :loading="busy" @click="saveWorkspace">
          保存
        </BaseButton>
      </template>
    </BaseModal>
  </div>
</template>
