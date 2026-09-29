<script setup lang="ts">
import type { UsageTimeRangeParams } from '../composables/useUsageTimeRange'
import type { UsageFilterDraft } from '../utils/filters'
import type { UsageAccountOption } from '@/api'
import { RotateCcw, Search, SlidersHorizontal, X } from '@lucide/vue'
import { onClickOutside, watchDebounced } from '@vueuse/core'
import { computed, onScopeDispose, ref, shallowRef, watch } from 'vue'
import { getUsageAccountOptions } from '@/api'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import UsageEntityFilter from './UsageEntityFilter.vue'

const props = defineProps<{ range: UsageTimeRangeParams, errors: boolean, error?: string }>()
const model = defineModel<UsageFilterDraft>({ required: true })
const expanded = ref(false)
const accountRoot = ref<HTMLElement>()
const accountQuery = ref('')
const accountOpen = ref(false)
const accountLoading = ref(false)
const accountError = ref(false)
const accounts = shallowRef<UsageAccountOption[]>([])
const labels = shallowRef<Record<string, UsageAccountOption>>({})
let controller: AbortController | undefined
let generation = 0
const selected = computed(() => (model.value.accountIds || model.value.accountId || '').split(',').filter(Boolean))
const activeCount = computed(() => Object.entries(model.value).filter(([key, value]) => value && key !== 'view').length)
function set(key: keyof UsageFilterDraft, value: string) {
  model.value = { ...model.value, [key]: value }
}
async function loadAccounts() {
  const current = ++generation
  controller?.abort()
  controller = new AbortController()
  accountLoading.value = true
  accountError.value = false
  try {
    const items = await getUsageAccountOptions({
      ...props.range,
      ...(model.value.startTime && model.value.endTime
        ? { startTime: model.value.startTime, endTime: model.value.endTime }
        : {}),
      search: accountQuery.value || undefined,
    }, { signal: controller.signal })
    if (current !== generation)
      return
    accounts.value = items
    labels.value = { ...labels.value, ...Object.fromEntries(items.map(item => [item.id, item])) }
  }
  catch {
    if (current === generation)
      accountError.value = true
  }
  finally {
    if (current === generation)
      accountLoading.value = false
  }
}
function closeAccounts() {
  accountOpen.value = false
  ++generation
  controller?.abort()
  accountLoading.value = false
}
function selectAccount(account: UsageAccountOption) {
  const ids = new Set(selected.value)
  if (ids.has(account.id))
    ids.delete(account.id)
  else if (ids.size < 50)
    ids.add(account.id)
  model.value = { ...model.value, accountId: '', accountIds: [...ids].join(','), accountSearch: '' }
}
function removeAccount(id: string) {
  model.value = { ...model.value, accountId: '', accountIds: selected.value.filter(value => value !== id).join(',') }
}
function accountLabel(id: string) {
  const account = labels.value[id]
  return account ? account.email || account.customName || account.name || id : id
}
function localTime(key: 'startTime' | 'endTime') {
  const value = model.value[key]
  if (!value || !Number.isFinite(Date.parse(value)))
    return ''
  const date = new Date(value)
  return new Date(date.getTime() - date.getTimezoneOffset() * 60_000).toISOString().slice(0, 16)
}
function setTime(key: 'startTime' | 'endTime', value: string) {
  set(key, value && Number.isFinite(Date.parse(value)) ? new Date(value).toISOString() : '')
}
watchDebounced(accountQuery, () => {
  if (accountOpen.value)
    void loadAccounts()
}, { debounce: 300 })
watch(accountOpen, (open) => {
  if (open)
    void loadAccounts()
})
watch([() => props.range, () => model.value.startTime, () => model.value.endTime], () => {
  if (accountOpen.value)
    void loadAccounts()
})
let labelController: AbortController | undefined
watch(selected, async (ids) => {
  labelController?.abort()
  const owned = new AbortController()
  labelController = owned
  const missing = ids.filter(id => !labels.value[id])
  // Bound restored-label requests independently of the live search.
  for (let offset = 0; offset < missing.length; offset += 5) {
    if (owned.signal.aborted)
      return
    await Promise.all(missing.slice(offset, offset + 5).map(async (id) => {
      try {
        const result = await getUsageAccountOptions({ ...props.range, search: id }, { signal: owned.signal })
        const account = result.find(item => item.id === id)
        if (!owned.signal.aborted && account)
          labels.value = { ...labels.value, [id]: account }
      }
      catch {}
    }))
  }
}, { immediate: true })
onClickOutside(accountRoot, closeAccounts)
onScopeDispose(() => {
  closeAccounts()
  labelController?.abort()
})

const textFields: { key: keyof UsageFilterDraft, label: string, placeholder?: string, type?: string }[] = [
  { key: 'accountSearch', label: '账号关键字', placeholder: '邮箱、备注或历史名称' },
  { key: 'upstreamModel', label: '上游模型', placeholder: '实际发送的模型' },
  { key: 'requestId', label: '请求 ID' },
  { key: 'upstreamRequestId', label: '上游请求 ID' },
  { key: 'responseId', label: 'Response ID' },
  { key: 'clientIp', label: '客户端 IP' },
  { key: 'clientStatusCode', label: '客户端状态码', type: 'number' },
  { key: 'upstreamStatusCode', label: '上游状态码', type: 'number' },
  { key: 'minLatencyMs', label: '总耗时下限 (ms)', type: 'number' },
  { key: 'maxLatencyMs', label: '总耗时上限 (ms)', type: 'number' },
  { key: 'minFirstTokenMs', label: '首字下限 (ms)', type: 'number' },
  { key: 'maxFirstTokenMs', label: '首字上限 (ms)', type: 'number' },
]
const errorFields: { key: keyof UsageFilterDraft, label: string, placeholder?: string }[] = [
  { key: 'failureKind', label: '错误分类', placeholder: '分类码，例如 request_timeout' },
  { key: 'errorCode', label: '上游错误码' },
  { key: 'errorPhase', label: '已记录的失败阶段', placeholder: '诊断详情中的 stage' },
]
const all = { label: '全部', value: '' }
const modeOptions = [all, { label: 'Excel', value: 'excel' }, { label: '原生 Codex', value: 'codex' }, { label: '未记录', value: 'unknown' }]
const transportOptions = [all, { label: 'HTTP', value: 'http' }, { label: 'HTTP / SSE', value: 'http_sse' }, { label: 'WebSocket', value: 'websocket' }, { label: '未记录', value: 'unknown' }]
const cacheOptions = [all, { label: '缓存命中', value: 'hit' }, { label: '未命中', value: 'miss' }, { label: '未记录', value: 'unknown' }]
const recoveryOptions = [all, { label: '尚未恢复', value: 'unrecovered' }, { label: '后续已恢复', value: 'recovered' }]
const scopeOptions = [{ label: '全部错误事件', value: '' }, { label: '请求最终错误', value: 'requests' }, { label: '各次尝试 / 运维事件', value: 'events' }]
const routeOptions = [all, { label: 'Responses', value: '/v1/responses' }, { label: 'Chat Completions', value: '/v1/chat/completions' }, { label: 'Compact', value: '/v1/responses/compact' }]
</script>

<template>
  <!-- eslint-disable vue-a11y/label-has-for -- BaseInput/BaseSelect render labelable controls within these labels. -->
  <section class="mb-5 min-w-0 border-y border-cp-border py-3" aria-label="请求筛选">
    <div class="grid min-w-0 grid-cols-1 items-end gap-3 sm:grid-cols-2 xl:grid-cols-4">
      <label class="min-w-0">
        <span class="mb-1 block text-cp-xs text-cp-text-secondary">搜索</span>
        <BaseInput :model-value="model.search || ''" placeholder="邮箱、备注、请求 ID、错误信息" aria-label="请求搜索" @update:model-value="set('search', $event)">
          <template #prefix><Search :size="16" /></template>
        </BaseInput>
      </label>
      <div ref="accountRoot" class="relative min-w-0">
        <label>
          <span class="mb-1 block text-cp-xs text-cp-text-secondary">账号</span>
          <BaseInput v-model="accountQuery" placeholder="搜索并选择账号" aria-label="搜索账号" aria-haspopup="listbox" :aria-expanded="accountOpen" @focusin="accountOpen = true" @keydown.esc="closeAccounts" />
        </label>
        <div v-if="accountOpen" class="absolute top-full right-0 left-0 z-40 mt-1 max-h-72 overflow-auto rounded-cp border border-cp-border bg-cp-bg-elevated p-1 shadow-cp" role="listbox" aria-label="账号候选" aria-multiselectable="true">
          <p v-if="accountLoading" class="px-2 text-cp-xs">
            加载中
          </p>
          <p v-else-if="accountError" role="alert" class="px-2 text-cp-xs text-cp-error">
            账号查询失败
          </p>
          <p v-else-if="!accounts.length" class="px-2 text-cp-xs text-cp-text-secondary">
            暂无匹配账号
          </p>
          <button v-for="account in accounts" :key="account.id" type="button" role="option" :aria-selected="selected.includes(account.id)" class="block w-full rounded-cp-sm border-0 bg-transparent p-2 text-left hover:bg-cp-bg-text-hover" @click="selectAccount(account)" @keydown.esc="closeAccounts">
            <span class="block break-all text-cp-sm" :class="selected.includes(account.id) ? 'font-bold text-cp-primary-text' : 'text-cp-text'">{{ account.email || account.name || account.id }}{{ account.deleted ? '（已删除）' : '' }}</span>
            <span class="block break-all text-cp-xs text-cp-text-secondary">{{ account.customName || account.name }} · {{ account.id }}</span>
          </button>
        </div>
      </div>
      <label class="min-w-0">
        <span class="mb-1 block text-cp-xs text-cp-text-secondary">请求模型</span>
        <BaseInput :model-value="model.requestedModel || ''" placeholder="精确模型名" aria-label="请求模型" @update:model-value="set('requestedModel', $event)" />
      </label>
      <div class="flex min-w-0 items-end gap-2">
        <label class="min-w-0 flex-1">
          <span class="mb-1 block text-cp-xs text-cp-text-secondary">上游模式</span>
          <BaseSelect :model-value="model.upstreamMode || ''" :options="modeOptions" aria-label="上游模式" class="w-full" @update:model-value="set('upstreamMode', $event)" />
        </label>
        <BaseIconButton label="高级筛选" :aria-expanded="expanded" @click="expanded = !expanded">
          <SlidersHorizontal :size="17" />
        </BaseIconButton>
        <BaseIconButton label="重置全部筛选" :disabled="activeCount === 0" @click="model = { view: model.view }; accountQuery = ''">
          <RotateCcw :size="17" />
        </BaseIconButton>
      </div>
    </div>
    <div v-if="selected.length" class="mt-2 flex flex-wrap gap-2" aria-label="已选账号">
      <span v-for="id in selected" :key="id" class="inline-flex max-w-full items-center gap-1 rounded-cp-sm bg-cp-bg-text-hover px-2 text-cp-xs">
        <span class="min-w-0 break-all">{{ accountLabel(id) }}</span>
        <BaseIconButton :label="`移除账号 ${accountLabel(id)}`" size="sm" @click="removeAccount(id)"><X :size="13" /></BaseIconButton>
      </span>
    </div>
    <div v-if="expanded" class="mt-3 grid min-w-0 grid-cols-1 gap-3 border-t border-cp-border pt-3 sm:grid-cols-2 xl:grid-cols-4">
      <UsageEntityFilter kind="key" :model-value="model.clientApiKeyId || ''" @update:model-value="set('clientApiKeyId', $event)" />
      <UsageEntityFilter kind="group" :model-value="model.groupId || ''" @update:model-value="set('groupId', $event)" />
      <label class="min-w-0"><span class="mb-1 block text-cp-xs text-cp-text-secondary">请求接口</span><BaseSelect class="w-full" :model-value="model.route || ''" :options="routeOptions" aria-label="请求接口" @update:model-value="set('route', $event)" /></label>
      <label v-for="key in (['startTime', 'endTime'] as const)" :key="key" class="min-w-0">
        <span class="mb-1 block text-cp-xs text-cp-text-secondary">{{ key === 'startTime' ? '开始时间' : '结束时间' }}</span>
        <BaseInput type="datetime-local" :model-value="localTime(key)" :aria-label="key === 'startTime' ? '开始时间' : '结束时间'" @update:model-value="setTime(key, $event)" />
      </label>
      <label v-for="field in textFields" :key="field.key" class="min-w-0">
        <span class="mb-1 block text-cp-xs text-cp-text-secondary">{{ field.label }}</span>
        <BaseInput :type="field.type || 'text'" :min="field.type === 'number' ? 0 : undefined" :model-value="model[field.key] || ''" :placeholder="field.placeholder" :aria-label="field.label" @update:model-value="set(field.key, $event)" />
      </label>
      <label class="min-w-0"><span class="mb-1 block text-cp-xs text-cp-text-secondary">上游传输</span><BaseSelect class="w-full" :model-value="model.upstreamTransport || ''" :options="transportOptions" aria-label="上游传输" @update:model-value="set('upstreamTransport', $event)" /></label>
      <label class="min-w-0"><span class="mb-1 block text-cp-xs text-cp-text-secondary">客户端传输</span><BaseSelect class="w-full" :model-value="model.clientTransport || ''" :options="transportOptions.filter(option => option.value !== 'unknown')" aria-label="客户端传输" @update:model-value="set('clientTransport', $event)" /></label>
      <label class="min-w-0"><span class="mb-1 block text-cp-xs text-cp-text-secondary">缓存</span><BaseSelect class="w-full" :model-value="model.cacheMatch || ''" :options="cacheOptions" aria-label="缓存筛选" @update:model-value="set('cacheMatch', $event)" /></label>
      <template v-if="errors">
        <label v-for="field in errorFields" :key="field.key" class="min-w-0">
          <span class="mb-1 block text-cp-xs text-cp-text-secondary">{{ field.label }}</span>
          <BaseInput :model-value="model[field.key] || ''" :placeholder="field.placeholder" :aria-label="field.label" @update:model-value="set(field.key, $event)" />
        </label>
        <label class="min-w-0"><span class="mb-1 block text-cp-xs text-cp-text-secondary">错误范围</span><BaseSelect class="w-full" :model-value="model.errorScope || ''" :options="scopeOptions" aria-label="错误范围" @update:model-value="set('errorScope', $event)" /></label>
        <label class="min-w-0"><span class="mb-1 block text-cp-xs text-cp-text-secondary">恢复状态</span><BaseSelect class="w-full" :model-value="model.recovery || ''" :options="recoveryOptions" aria-label="恢复状态" @update:model-value="set('recovery', $event)" /></label>
      </template>
    </div>
    <p v-if="error" role="alert" class="mb-0 text-cp-sm text-cp-error">
      {{ error }}
    </p>
  </section>
</template>
