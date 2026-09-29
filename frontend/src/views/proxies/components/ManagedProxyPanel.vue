<script setup lang="ts">
import type { CountryFilter, MihomoCommand, MihomoNode, MihomoStatus, NodeCheck } from '@/api/modules/mihomo'
import { Activity, Download, FileUp, Pencil, Play, Plus, RefreshCw, Search, ShieldCheck, Square, Trash2, Wifi } from '@lucide/vue'
import { computed, onMounted, onScopeDispose, ref, shallowRef, watch } from 'vue'
import { checkMihomoNode, getMihomo, updateMihomo } from '@/api/modules/mihomo'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import { toast } from '@/components/base/BaseToast'
import { errorMessage } from '@/utils/async'
import { formatDateTime } from '@/utils/date'
import { batchTargets, checkOutcome, filterNodes, runNodeBatch } from '../mihomoNodes'
import { countryBlockReason, qualityStatus, regionLabel, regionMatches, sameCountryFilter } from '../mihomoPresentation'

const props = defineProps<{ tab: string }>()
const state = shallowRef<MihomoStatus>()
const error = ref('')
const loading = ref(false)
const submitting = ref(false)
const input = ref('')
const label = ref('')
const search = ref('')
const nodeSource = ref('all')
const nodeState = ref('all')
const page = ref(1)
const selectedNodes = ref(new Set<string>())
const checking = ref(new Set<string>())
const results = ref<Record<string, NodeCheck>>({})
const checkErrors = ref<Record<string, string>>({})
const batchRunning = ref(false)
const batch = ref({ quality: false, total: 0, completed: 0, passed: 0, warning: 0, challenge: 0, failed: 0, skipped: 0 })
const pending = shallowRef<MihomoCommand>()
const confirmOpen = ref(false)
const editOpen = ref(false)
const editing = shallowRef<{ id: string, label: string }>()
const editName = ref('')
const editUrl = ref('')
const reportOpen = ref(false)
const report = shallowRef<NodeCheck>()
const rules = ref<CountryFilter>({ mode: 'off', codes: [], allowUnknown: false, dynamicProviderManaged: false })
const countrySearch = ref('')
const rulesDirty = ref(false)
let disposed = false
let timer: ReturnType<typeof setTimeout> | undefined
let abort: AbortController | undefined
const busy = computed(() => submitting.value || state.value?.busy)
const nodes = computed(() => filterNodes(state.value?.nodeStates ?? [], {
  dynamicOnly: props.tab === 'dynamic',
  source: nodeSource.value,
  state: nodeState.value,
  search: search.value,
}))
const sourceOptions = computed(() => [
  { value: 'all', label: '所有来源' },
  { value: 'subscription', label: '全部订阅节点' },
  { value: 'dynamic', label: '动态代理' },
  ...(state.value?.subscriptionItems ?? []).map(item => ({ value: `source:${item.id}`, label: item.label })),
])
const totalPages = computed(() => Math.max(1, Math.ceil(nodes.value.length / 50)))
const pagedNodes = computed(() => nodes.value.slice((page.value - 1) * 50, page.value * 50))
const pagination = computed(() => ({ currentPage: page.value, pageSize: 50, pageSizes: [50], total: nodes.value.length }))
const targets = computed(() => batchTargets(nodes.value, selectedNodes.value))
const pageSelected = computed(() => pagedNodes.value.length > 0 && pagedNodes.value.every(node => selectedNodes.value.has(node.name)))
const pageIndeterminate = computed(() => !pageSelected.value && pagedNodes.value.some(node => selectedNodes.value.has(node.name)))
const batchSummary = computed(() => {
  const value = batch.value
  return `通过 ${value.passed} · 失败 ${value.failed}${value.quality ? ` · 告警 ${value.warning} · 挑战 ${value.challenge}` : ''}${value.skipped ? ` · 跳过 ${value.skipped}` : ''}`
})
watch([search, nodeSource, nodeState, () => props.tab], () => {
  page.value = 1
  selectedNodes.value.clear()
})
watch(totalPages, (count) => {
  page.value = Math.min(page.value, count)
})
const countries = computed(() => (state.value?.countryCodes ?? []).filter(c => regionMatches(c, countrySearch.value)))
const countryProgress = computed(() => {
  const subscriptionNodes = (state.value?.nodeStates ?? []).filter(node => !node.dynamic)
  return {
    total: subscriptionNodes.length,
    known: subscriptionNodes.filter(node => node.countryCode).length,
    unchecked: subscriptionNodes.filter(node => !node.countryCode && !node.countryCheckedAt && !node.countryError).length,
    failed: subscriptionNodes.filter(node => !node.countryCode && node.countryError).length,
  }
})
const counts = computed(() => [
  { name: 'Codex · Mihomo', value: state.value?.codexWarmPool },
  { name: 'Codex · 普通代理', value: state.value?.codexIpWarmPool },
  { name: 'Excel · Mihomo', value: state.value?.bpsWarmPool },
  { name: 'Excel · 普通代理', value: state.value?.bpsIpWarmPool },
])
function lines() {
  return input.value.split(/\r?\n/).map(s => s.trim()).filter(Boolean)
}
async function load() {
  if (loading.value || submitting.value || disposed)
    return
  loading.value = true
  const owner = new AbortController()
  abort = owner
  try {
    const value = await getMihomo({ silent: true, signal: owner.signal })
    if (disposed || owner.signal.aborted)
      return
    state.value = value
    error.value = ''
    if (!rulesDirty.value || sameCountryFilter(value.countryFilter, rules.value)) {
      rules.value = { ...value.countryFilter, codes: [...value.countryFilter.codes] }
      rulesDirty.value = false
    }
  }
  catch (cause) {
    if (!disposed && !owner.signal.aborted)
      error.value = errorMessage(cause)
  }
  finally { loading.value = false }
}
async function poll() {
  await load()
  if (!disposed)
    timer = setTimeout(poll, state.value?.busy ? 1000 : 5000)
}
async function execute(command: MihomoCommand) {
  if (busy.value)
    return
  submitting.value = true
  abort?.abort()
  try {
    const result = await updateMihomo(command)
    if (disposed)
      return
    state.value = result
    error.value = ''
    if (command.action === 'subscription_add' || command.action.startsWith('dynamic_'))
      input.value = ''
    editOpen.value = false
  }
  catch (cause) { error.value = errorMessage(cause) }
  finally { submitting.value = false }
}
function confirm(command: MihomoCommand) {
  pending.value = command
  confirmOpen.value = true
}
async function confirmed() {
  if (pending.value)
    await execute(pending.value)
  confirmOpen.value = false
}
function edit(source: { id: string, label: string }) {
  editing.value = source
  editName.value = source.label
  editUrl.value = ''
  editOpen.value = true
}
function toggleNode(name: string, selected: boolean) {
  if (selected)
    selectedNodes.value.add(name)
  else
    selectedNodes.value.delete(name)
}
function togglePage(selected: boolean) {
  for (const node of pagedNodes.value)
    toggleNode(node.name, selected)
}
function nodeSourceLabel(node: MihomoNode) {
  return node.dynamic ? '动态代理' : node.subscriptionIds.map(id => state.value?.subscriptionItems.find(item => item.id === id)?.label ?? id).join('、') || '订阅（待更新来源）'
}
async function runCheck(node: MihomoNode, quality: boolean, silent = false) {
  if (disposed || checking.value.has(node.name))
    return
  checking.value.add(node.name)
  delete checkErrors.value[node.name]
  try {
    const result = await checkMihomoNode(node.name, quality, { silent })
    if (disposed)
      return
    results.value[node.name] = result
    return result
  }
  catch (cause) {
    if (!disposed)
      checkErrors.value[node.name] = errorMessage(cause)
    throw cause
  }
  finally { checking.value.delete(node.name) }
}
async function check(node: MihomoNode, quality: boolean) {
  if (batchRunning.value)
    return
  try {
    const result = await runCheck(node, quality)
    if (!result || disposed)
      return
    if (quality) {
      report.value = result
      reportOpen.value = true
    }
    else {
      toast[result.success ? 'success' : 'warning'](result.message ?? '节点检测结束')
    }
  }
  catch (cause) {
    if (!disposed)
      error.value = errorMessage(cause)
  }
}
async function checkBatch(quality: boolean) {
  if (batchRunning.value || !targets.value.length)
    return
  // Freeze the scope: changing filters during a batch only affects the next run.
  const snapshot = [...targets.value]
  batchRunning.value = true
  batch.value = { quality, total: snapshot.length, completed: 0, passed: 0, warning: 0, challenge: 0, failed: 0, skipped: 0 }
  try {
    await runNodeBatch(snapshot, quality ? 2 : 3, async (node) => {
      const result = await runCheck(node, quality, true)
      return result ? checkOutcome(result, quality) : 'skipped'
    }, (outcome) => {
      batch.value[outcome]++
      batch.value.completed++
    }, () => disposed)
    if (!disposed) {
      const summary = `${quality ? '批量质量检测' : '批量连接检测'}完成：${batchSummary.value}`
      toast[batch.value.failed || batch.value.warning || batch.value.challenge || batch.value.skipped ? 'warning' : 'success'](summary)
    }
  }
  finally { batchRunning.value = false }
}
function showReport(node: MihomoNode) {
  report.value = results.value[node.name] ?? node.check ?? undefined
  reportOpen.value = true
}
async function importFile(event: Event) {
  const target = event.target as HTMLInputElement
  const file = target.files?.[0]
  if (file)
    input.value = [input.value, await file.text()].filter(Boolean).join('\n')
  target.value = ''
}
function toggleCountry(code: string) {
  rules.value.codes = rules.value.codes.includes(code) ? rules.value.codes.filter(c => c !== code) : [...rules.value.codes, code]
  rulesDirty.value = true
}
onMounted(poll)
onScopeDispose(() => {
  disposed = true
  if (timer)
    clearTimeout(timer)
  abort?.abort()
})
</script>

<template>
  <section class="min-h-0 flex-1 overflow-auto py-5">
    <div class="mb-4 flex flex-wrap items-center gap-3 border-b border-cp-border pb-4">
      <span class="text-cp font-medium">Mihomo {{ state?.version }}</span>
      <span class="text-cp-sm" :class="state?.running ? 'text-green-600' : 'text-cp-text-secondary'">{{ state?.running ? '运行中' : state?.installed ? '已停止' : '未安装' }}</span>
      <span v-if="state?.busy" class="text-cp-sm text-cp-text-secondary">处理中 · {{ state.phase }}</span>
      <BaseIconButton class="ml-auto" label="刷新受管代理状态" :loading="loading" @click="load">
        <RefreshCw class="size-4" />
      </BaseIconButton>
    </div>
    <p v-if="error || state?.error" role="alert" class="mb-4 break-words rounded border border-red-300 p-3 text-cp-sm text-red-600">
      {{ error || state?.error }}
    </p>

    <template v-if="tab === 'subscriptions'">
      <div class="mb-6 grid gap-3">
        <BaseFormItem label="订阅名称（选填）">
          <BaseInput v-model="label" />
        </BaseFormItem>
        <BaseFormItem label="订阅地址（每行一个）">
          <textarea v-model="input" aria-label="订阅地址（每行一个）" rows="4" class="w-full rounded border border-cp-border bg-cp-surface p-3 text-cp-sm" spellcheck="false" />
        </BaseFormItem>
        <div class="flex flex-wrap items-center gap-3">
          <BaseButton variant="primary" :disabled="busy || !lines().length" @click="execute({ action: 'subscription_add', name: label, subscriptions: lines() })">
            <Plus class="mr-2 size-4" />添加订阅
          </BaseButton>
          <label for="mihomo-subscription-file" class="flex cursor-pointer items-center gap-2 text-cp-sm"><FileUp class="size-4" />导入 TXT<input id="mihomo-subscription-file" type="file" accept=".txt,text/plain" class="sr-only" @change="importFile"></label>
          <BaseSelect class="sm:ml-auto sm:w-64" :model-value="state?.subscriptionDownloadMode ?? 'auto'" :disabled="busy" :options="[{ value: 'auto', label: '下载订阅：自动' }, { value: 'proxy', label: '下载订阅：仅代理' }, { value: 'direct', label: '下载订阅：直连' }]" @update:model-value="execute({ action: 'download_mode', downloadMode: $event })" />
        </div>
      </div>
      <div class="overflow-x-auto">
        <table class="w-full whitespace-nowrap text-left text-cp-sm">
          <thead class="border-b border-cp-border text-cp-text-secondary">
            <tr>
              <th class="p-3">
                订阅
              </th><th class="p-3">
                状态
              </th><th class="p-3">
                节点
              </th><th class="p-3">
                更新时间
              </th><th class="p-3">
                操作
              </th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="source in state?.subscriptionItems" :key="source.id" class="border-b border-cp-border">
              <td class="p-3">
                {{ source.label }}
              </td><td class="p-3">
                {{ source.enabled ? '启用' : '停用' }} · {{ source.cached ? '有缓存' : '无缓存' }}
              </td><td class="p-3">
                {{ source.nodes }}
              </td><td class="p-3">
                {{ source.updatedAt ? formatDateTime(source.updatedAt) : '未更新' }}
              </td>
              <td class="p-3">
                <div class="flex gap-1">
                  <BaseIconButton label="编辑订阅" :disabled="busy" @click="edit(source)">
                    <Pencil class="size-4" />
                  </BaseIconButton>
                  <BaseIconButton label="刷新此订阅" :disabled="busy || !source.enabled" @click="execute({ action: 'subscription_refresh', target: source.id })">
                    <RefreshCw class="size-4" />
                  </BaseIconButton>
                  <BaseIconButton :label="source.enabled ? '停用订阅' : '启用订阅'" :disabled="busy" @click="execute({ action: source.enabled ? 'subscription_disable' : 'subscription_enable', target: source.id })">
                    <Square v-if="source.enabled" class="size-4" /><Play v-else class="size-4" />
                  </BaseIconButton>
                  <BaseIconButton label="删除订阅" :disabled="busy" @click="confirm({ action: 'subscription_remove', target: source.id })">
                    <Trash2 class="size-4" />
                  </BaseIconButton>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
        <p v-if="!state?.subscriptionItems.length" class="py-10 text-center text-cp-sm text-cp-text-secondary">
          暂无订阅
        </p>
      </div>
    </template>

    <div v-if="tab === 'dynamic'" class="mb-6 grid gap-3">
      <BaseFormItem label="动态代理（每行一个）">
        <textarea v-model="input" aria-label="动态代理（每行一个）" rows="5" class="w-full rounded border border-cp-border bg-cp-surface p-3 text-cp-sm" placeholder="host:port:username:password" spellcheck="false" />
      </BaseFormItem>
      <div class="flex flex-wrap gap-2">
        <BaseButton variant="primary" :disabled="busy || !lines().length" @click="execute({ action: 'dynamic_append', dynamicProxies: lines() })">
          <Plus class="mr-2 size-4" />追加
        </BaseButton>
        <BaseButton :disabled="busy || !lines().length" @click="confirm({ action: 'dynamic_replace', dynamicProxies: lines() })">
          替换全部
        </BaseButton>
        <BaseButton :disabled="busy || !state?.dynamicProxies" @click="confirm({ action: 'dynamic_clear' })">
          清空
        </BaseButton>
        <label for="mihomo-dynamic-file" class="flex cursor-pointer items-center gap-2 text-cp-sm"><FileUp class="size-4" />导入 TXT<input id="mihomo-dynamic-file" type="file" accept=".txt,text/plain" class="sr-only" @change="importFile"></label>
      </div>
    </div>

    <template v-if="tab === 'nodes' || tab === 'dynamic'">
      <div class="mb-4 flex flex-wrap gap-3">
        <BaseInput v-model="search" class="min-w-40 flex-1" placeholder="搜索节点、地区" aria-label="搜索受管节点">
          <template #prefix>
            <Search class="size-4" />
          </template>
        </BaseInput>
        <BaseSelect v-if="tab === 'nodes'" v-model="nodeSource" class="w-48" aria-label="节点来源" :options="sourceOptions" />
        <BaseSelect v-model="nodeState" class="w-40" aria-label="节点状态" :options="[{ value: 'all', label: '所有状态' }, { value: 'ready', label: '已启用' }, { value: 'disabled', label: '已停用' }, { value: 'failed', label: '探测失败' }, { value: 'blocked', label: '地区未准入' }]" />
      </div>
      <div class="mb-3 flex flex-wrap items-center gap-3">
        <BaseButton :disabled="batchRunning || !targets.length" @click="checkBatch(false)">
          <Wifi class="mr-2 size-4" />批量测试连接
        </BaseButton>
        <BaseButton :disabled="batchRunning || !targets.length" @click="checkBatch(true)">
          <ShieldCheck class="mr-2 size-4" />批量质量检测
        </BaseButton>
        <span class="text-cp-sm text-cp-text-secondary" data-node-selection>{{ selectedNodes.size ? `已选 ${selectedNodes.size} · 本次检测 ${targets.length}` : `未勾选 · 检测全部筛选结果 ${nodes.length} 个` }}</span>
        <BaseButton v-if="selectedNodes.size" size="sm" @click="selectedNodes.clear()">
          清空选择
        </BaseButton>
      </div>
      <div v-if="batch.total" role="status" class="mb-3 flex flex-wrap gap-x-4 gap-y-1 text-cp-sm text-cp-text-secondary" data-node-batch>
        <span>{{ batch.quality ? '质量检测' : '连接检测' }}{{ batchRunning ? '中' : '完成' }} {{ batch.completed }}/{{ batch.total }} · {{ batch.quality ? 2 : 3 }} 并发</span>
        <span>{{ batchSummary }}</span>
      </div>
      <div class="overflow-x-auto">
        <table class="w-full min-w-170 whitespace-nowrap text-left text-cp-sm">
          <thead class="whitespace-nowrap border-b border-cp-border text-cp-text-secondary">
            <tr>
              <th class="w-10 p-3">
                <BaseCheckbox label="选择本页全部节点" :model-value="pageSelected" :indeterminate="pageIndeterminate" :disabled="!pagedNodes.length" @update:model-value="togglePage" />
              </th>
              <th class="p-3">
                节点
              </th><th class="p-3">
                来源 / 状态
              </th><th class="p-3">
                出口 / 地区
              </th><th class="p-3">
                检测
              </th><th class="p-3">
                操作
              </th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="node in pagedNodes" :key="node.name" class="border-b border-cp-border">
              <td class="p-3">
                <BaseCheckbox :label="`选择节点 ${node.displayName}`" :model-value="selectedNodes.has(node.name)" @update:model-value="toggleNode(node.name, $event)" />
              </td>
              <td class="max-w-72 break-words p-3">
                <div>{{ node.displayName }}</div><span class="text-cp-xs text-cp-text-tertiary">{{ node.name }}</span>
              </td>
              <td class="p-3">
                <div class="max-w-48 truncate" :title="nodeSourceLabel(node)">
                  {{ nodeSourceLabel(node) }}
                </div>
                {{ node.state === 'ready' ? '已启用' : node.state === 'disabled' ? '停用' : '失败' }}<div v-if="node.countryBlocked" class="text-cp-warning-on-container" data-country-block-reason>
                  {{ countryBlockReason(node, state?.countryFilter) }}
                </div>
              </td>
              <td class="p-3">
                {{ (results[node.name] ?? node.check)?.exitIp ?? '未检测' }}<div class="text-cp-xs text-cp-text-secondary">
                  规则地区：{{ node.dynamic ? '供应商动态地区' : node.countryCode ? regionLabel(node.countryCode) : '未知' }}
                </div>
                <div v-if="node.countryCheckedAt && !node.dynamic" class="text-cp-xs text-cp-text-tertiary">
                  地区检测：{{ formatDateTime(node.countryCheckedAt) }}
                </div>
                <div v-if="(results[node.name] ?? node.check)?.countryName" class="text-cp-xs text-cp-text-secondary">
                  连接诊断：{{ [(results[node.name] ?? node.check)?.countryName, (results[node.name] ?? node.check)?.region, (results[node.name] ?? node.check)?.city].filter(Boolean).join(' / ') }}
                </div>
              </td>
              <td class="whitespace-nowrap p-3">
                <span v-if="checking.has(node.name)">检测中</span><template v-else>
                  <div v-if="checkErrors[node.name]" class="max-w-64 truncate text-cp-error" :title="checkErrors[node.name]">
                    检测失败：{{ checkErrors[node.name] }}
                  </div>
                  {{ (results[node.name] ?? node.check)?.latencyMs ?? '-' }} ms<button v-if="(results[node.name] ?? node.check)?.quality" class="block text-cp-primary" @click="showReport(node)">
                    {{ qualityStatus((results[node.name] ?? node.check)?.quality) }} · {{ (results[node.name] ?? node.check)?.quality?.grade }} · {{ (results[node.name] ?? node.check)?.quality?.score }}
                  </button>
                </template>
              </td>
              <td class="p-3">
                <div class="flex gap-1">
                  <BaseIconButton label="测试连接（不修改状态）" :disabled="batchRunning || checking.has(node.name)" @click="check(node, false)">
                    <Wifi class="size-4" />
                  </BaseIconButton>
                  <BaseIconButton label="完整质量检测" :disabled="batchRunning || checking.has(node.name)" @click="check(node, true)">
                    <ShieldCheck class="size-4" />
                  </BaseIconButton>
                  <BaseIconButton label="探测并更新节点状态" :disabled="busy" @click="execute({ action: 'probe', target: node.name })">
                    <Activity class="size-4" />
                  </BaseIconButton>
                  <BaseIconButton :label="node.state === 'ready' ? '停用节点' : '恢复节点'" :disabled="busy" @click="execute({ action: node.state === 'ready' ? 'disable' : 'recover', target: node.name })">
                    <Square v-if="node.state === 'ready'" class="size-4" /><Play v-else class="size-4" />
                  </BaseIconButton>
                  <BaseIconButton v-if="!node.dynamic" label="检测地区" :disabled="busy" @click="execute({ action: 'country_probe', target: node.name })">
                    <RefreshCw class="size-4" />
                  </BaseIconButton>
                  <BaseIconButton v-if="node.dynamic" label="删除动态代理" :disabled="busy" @click="confirm({ action: 'dynamic_remove', target: node.name })">
                    <Trash2 class="size-4" />
                  </BaseIconButton>
                </div>
              </td>
            </tr>
          </tbody>
        </table><p v-if="!nodes.length" class="py-10 text-center text-cp-text-secondary">
          暂无匹配节点
        </p>
      </div>
      <BaseTablePagination :pagination="pagination" :loading="false" @page-change="page = $event" />
    </template>

    <template v-if="tab === 'kernel'">
      <div class="mb-6 flex flex-wrap gap-3">
        <BaseButton :disabled="busy || state?.installed || !state?.supported" @click="execute({ action: 'install' })">
          <Download class="mr-2 size-4" />安装 {{ state?.version }}
        </BaseButton>
        <BaseButton :disabled="busy || !state?.installed || state?.running" @click="execute({ action: 'start' })">
          <Play class="mr-2 size-4" />启动
        </BaseButton>
        <BaseButton :disabled="busy || !state?.running" @click="confirm({ action: 'stop' })">
          <Square class="mr-2 size-4" />停止
        </BaseButton>
        <span v-if="state && !state.supported" class="self-center text-cp-sm text-cp-text-secondary">需要 Linux amd64 / arm64</span>
      </div>
      <div class="mb-6 grid gap-4 border-y border-cp-border py-4 sm:grid-cols-2">
        <div v-for="pool in counts" :key="pool.name">
          <h3 class="m-0 mb-2 text-cp font-medium">
            {{ pool.name }}
          </h3><div class="text-cp-sm">
            就绪 {{ pool.value?.ready ?? 0 }} / 目标 {{ pool.value?.target ?? 0 }} · 检测 {{ pool.value?.checking ?? 0 }} · 冷却 {{ pool.value?.cooling ?? 0 }}
          </div><div v-for="(count, reason) in pool.value?.failureReasons" :key="reason" class="mt-1 text-cp-xs text-cp-text-secondary">
            {{ reason }}：{{ count }}
          </div>
        </div>
      </div>
      <div class="max-w-3xl space-y-4">
        <p class="text-cp-sm text-cp-text-secondary" data-country-progress>
          订阅节点地区：已确认 {{ countryProgress.known }} / {{ countryProgress.total }} · 未检测 {{ countryProgress.unchecked }} · 检测失败 {{ countryProgress.failed }}
        </p>
        <BaseFormItem label="地区规则">
          <BaseSelect :model-value="rules.mode" :disabled="busy" :options="[{ value: 'off', label: '不过滤' }, { value: 'include', label: '只使用所选地区' }, { value: 'exclude', label: '排除所选地区' }]" @update:model-value="(v) => { if (v === 'off' || v === 'include' || v === 'exclude') { rules.mode = v; rulesDirty = true } }" />
        </BaseFormItem>
        <p class="text-cp-xs text-cp-text-secondary">
          地区规则采用独立的出口地区检测结果，不按节点名称或连接诊断中的位置判定。未检测或检测失败的节点按未知地区处理。
        </p>
        <div v-if="rules.codes.length" class="flex flex-wrap gap-2 text-cp-xs" aria-label="已选地区">
          <span v-for="code in rules.codes" :key="code" class="rounded border border-cp-border px-2 py-1">{{ regionLabel(code) }}</span>
        </div>
        <BaseInput v-model="countrySearch" placeholder="搜索地区名称或代码，例如 新加坡、Singapore、SG" aria-label="搜索地区代码" />
        <div class="grid max-h-52 grid-cols-2 gap-2 overflow-y-auto rounded border border-cp-border p-3 sm:grid-cols-4">
          <BaseCheckbox v-for="code in countries" :key="code" :model-value="rules.codes.includes(code)" :label="regionLabel(code)" :disabled="busy" show-label @update:model-value="toggleCountry(code)" />
        </div>
        <div class="flex flex-wrap gap-4">
          <BaseCheckbox v-model="rules.allowUnknown" label="允许未知地区" :disabled="busy" show-label @update:model-value="rulesDirty = true" />
          <BaseCheckbox v-model="rules.dynamicProviderManaged" label="动态代理地区由供应商管理" :disabled="busy" show-label @update:model-value="rulesDirty = true" />
        </div>
        <p class="text-cp-xs text-cp-text-secondary">
          允许未知地区会放行未检测或检测失败的节点，不能保证排除所选地区。动态代理每次连接可能换地区，供应商管理开启后不应用此处地区筛选。
        </p>
        <div class="flex flex-wrap gap-3">
          <BaseButton variant="primary" :disabled="busy || (rules.mode !== 'off' && !rules.codes.length)" @click="execute({ action: 'country_filter', countryFilter: { ...rules, codes: [...rules.codes] } })">
            保存地区规则
          </BaseButton><BaseButton :disabled="busy" @click="execute({ action: 'country_scan' })">
            检测下一批地区（最多20个）
          </BaseButton>
          <span v-if="rulesDirty" class="self-center text-cp-xs text-cp-text-secondary" role="status">{{ busy ? '地区规则等待确认' : '地区规则草稿未保存' }}</span>
        </div>
      </div>
    </template>

    <BaseConfirmModal v-model="confirmOpen" title="确认修改受管代理" destructive :loading="submitting" @confirm="confirmed">
      <p>确认执行此操作？正在使用被删除或停用出口的会话可能受到影响。</p>
    </BaseConfirmModal>
    <BaseModal v-model="editOpen" title="编辑订阅">
      <div class="space-y-4">
        <BaseFormItem label="名称">
          <BaseInput v-model="editName" />
        </BaseFormItem><BaseButton :disabled="busy" @click="execute({ action: 'subscription_rename', target: editing?.id, name: editName })">
          保存名称
        </BaseButton><BaseFormItem label="替换订阅地址">
          <BaseInput v-model="editUrl" type="password" />
        </BaseFormItem><BaseButton :disabled="busy || !editUrl.trim()" @click="execute({ action: 'subscription_update', target: editing?.id, subscriptions: [editUrl.trim()] })">
          更新地址并刷新
        </BaseButton>
      </div>
    </BaseModal>
    <BaseModal v-model="reportOpen" title="节点质量报告">
      <div v-if="report?.quality" class="space-y-3">
        <div class="font-medium">
          {{ qualityStatus(report.quality) }} · {{ report.quality.grade }} · {{ report.quality.score }} 分
        </div>
        <p class="text-cp-xs text-cp-text-secondary">
          这是无账号凭据的连通性检测，不验证登录、模型权限或实际生成。OpenAI / Grok 的 HTTP 401 是预期鉴权响应，表示目标可达，不是模型调用成功。
        </p>
        <p class="text-cp-xs text-cp-text-tertiary">
          检测时间：{{ formatDateTime(report.quality.checkedAt) }}
        </p>
        <p>{{ report.quality.summary }}</p><div v-for="item in report.quality.checks" :key="item.name" class="border-t border-cp-border py-2 text-cp-sm">
          <div class="flex justify-between gap-3">
            <span>{{ item.name }} · {{ item.status }}</span><span>{{ item.latencyMs }} ms</span>
          </div><p class="my-1 break-words text-cp-text-secondary">
            {{ item.reason }}
          </p><span v-if="item.cfRay" class="text-cp-xs">CF-Ray：{{ item.cfRay }}</span>
        </div>
      </div>
    </BaseModal>
  </section>
</template>
