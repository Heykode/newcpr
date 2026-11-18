<script setup lang="ts">
import type { UsageTimeRangeParams } from '../composables/useUsageTimeRange'
import type { UsageFilterDraft } from '../utils/filters'
import type { UsageAccountOption } from '@/api'
import { RotateCcw, Search, SlidersHorizontal, X } from '@lucide/vue'
import { onClickOutside, watchDebounced } from '@vueuse/core'
import { computed, onScopeDispose, ref, shallowRef, useId, watch } from 'vue'
import { getUsageAccountOptions } from '@/api'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { t } from '@/i18n'
import UsageEntityFilter from './UsageEntityFilter.vue'
import UsageTimeRangePicker from './UsageTimeRangePicker.vue'

const props = defineProps<{ range: UsageTimeRangeParams, errors: boolean, error?: string }>()
const model = defineModel<UsageFilterDraft>({ required: true })
const expanded = ref(false)
const moreId = useId()
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
const activeCount = computed(() => Object.entries(model.value).filter(([key, value]) => value && key !== 'view' && !(key === 'timeRange' && value === 'today')).length)
function set(key: keyof UsageFilterDraft, value: string | undefined) {
  model.value = { ...model.value, [key]: value ?? '' }
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
  return account ? account.customName || account.name || account.email || id : id
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
  { key: 'accountSearch', label: '账号名称', placeholder: '请求时的账号名称或邮箱' },
  { key: 'clientStatusCode', label: '客户端状态码', type: 'number' },
]
// Older URLs and table drill-downs retain these filters, with visible removal controls.
const retiredFields: { key: keyof UsageFilterDraft, label: string }[] = [
  { key: 'upstreamModel', label: '上游模型' },
  { key: 'requestId', label: '请求 ID' },
  { key: 'upstreamRequestId', label: '上游请求 ID' },
  { key: 'responseId', label: 'Response ID' },
  { key: 'clientIp', label: '客户端 IP' },
  { key: 'upstreamStatusCode', label: '上游状态码' },
  { key: 'minLatencyMs', label: '总耗时下限 (ms)' },
  { key: 'maxLatencyMs', label: '总耗时上限 (ms)' },
  { key: 'minFirstTokenMs', label: '首响应下限 (ms)' },
  { key: 'maxFirstTokenMs', label: '首响应上限 (ms)' },
]
const retiredConditions = computed(() => retiredFields.filter(field => model.value[field.key]))
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
const conditionFields: { key: keyof UsageFilterDraft, label: string, options?: { label: string, value: string }[] }[] = [
  { key: 'search', label: '搜索' },
  { key: 'requestedModel', label: '请求模型' },
  { key: 'upstreamMode', label: '上游模式', options: modeOptions },
  { key: 'clientApiKeyId', label: 'API Key' },
  { key: 'groupId', label: '请求分组' },
  { key: 'route', label: '请求接口', options: routeOptions },
  ...textFields,
  { key: 'upstreamTransport', label: '上游传输', options: transportOptions },
  { key: 'clientTransport', label: '客户端传输', options: transportOptions },
  { key: 'cacheMatch', label: '缓存', options: cacheOptions },
  ...errorFields,
  { key: 'errorScope', label: '错误范围', options: scopeOptions },
  { key: 'recovery', label: '恢复状态', options: recoveryOptions },
]
const activeConditions = computed(() => conditionFields.filter(field => model.value[field.key]))
const moreCount = computed(() => activeConditions.value.filter(field => !['search', 'requestedModel', 'upstreamMode'].includes(field.key)).length)
function conditionText(field: typeof conditionFields[number]) {
  return field.options?.find(option => option.value === model.value[field.key])?.label || model.value[field.key]
}
</script>

<template>
  <!-- eslint-disable vue-a11y/label-has-for -- BaseInput/BaseSelect render labelable controls within these labels. -->
  <section class="min-w-0" aria-label="请求筛选">
    <div class="usage-filter-fields">
      <UsageTimeRangePicker v-model="model" :range="range" class="usage-filter-time" />
      <label class="usage-filter-search">
        <span class="mb-1 block text-cp-xs text-cp-text-secondary">搜索</span>
        <BaseInput clearable :model-value="model.search || ''" placeholder="邮箱、备注、请求 ID、错误信息" aria-label="请求搜索" @update:model-value="set('search', $event)">
          <template #prefix><Search :size="16" /></template>
        </BaseInput>
      </label>
      <div ref="accountRoot" class="relative usage-filter-account">
        <label>
          <span class="mb-1 block text-cp-xs text-cp-text-secondary">{{ t('usage.filters.account') }}</span>
          <BaseInput v-model="accountQuery" placeholder="搜索并选择账号" :aria-label="t('accounts.filters.searchLabel')" aria-haspopup="listbox" :aria-expanded="accountOpen" @focusin="accountOpen = true" @keydown.esc="closeAccounts" />
        </label>
        <div v-if="accountOpen" class="absolute top-full right-0 left-0 z-40 mt-1 max-h-72 overflow-auto rounded-cp border border-cp-border bg-cp-bg-elevated p-1 shadow-cp" role="listbox" aria-label="账号候选" aria-multiselectable="true">
          <p v-if="accountLoading" class="px-2 text-cp-xs">
            {{ t('common.status.loading') }}
          </p>
          <p v-else-if="accountError" role="alert" class="px-2 text-cp-xs text-cp-error">
            账号查询失败
          </p>
          <p v-else-if="!accounts.length" class="px-2 text-cp-xs text-cp-text-secondary">
            {{ t('appearance.preview.components.noMatchingAccountYet') }}
          </p>
          <button v-for="account in accounts" :key="account.id" type="button" role="option" :aria-selected="selected.includes(account.id)" class="block w-full rounded-cp-sm border-0 bg-transparent p-2 text-left hover:bg-cp-bg-text-hover" @click="selectAccount(account)" @keydown.esc="closeAccounts">
            <span class="block break-all text-cp-sm" :class="selected.includes(account.id) ? 'font-bold text-cp-primary-text' : 'text-cp-text'">{{ account.email || account.name || account.id }}{{ account.deleted ? '（已删除）' : '' }}</span>
            <span class="block break-all text-cp-xs text-cp-text-secondary">{{ account.customName || account.name }} · {{ account.id }}</span>
          </button>
        </div>
      </div>
      <label class="min-w-0">
        <span class="mb-1 block text-cp-xs text-cp-text-secondary">{{ t('usage.error.requestModel') }}</span>
        <BaseInput :model-value="model.requestedModel || ''" placeholder="精确模型名" :aria-label="t('usage.error.requestModel')" @update:model-value="set('requestedModel', $event)" />
      </label>
      <label class="min-w-0">
        <span class="mb-1 block text-cp-xs text-cp-text-secondary">上游模式</span>
        <BaseSelect :model-value="model.upstreamMode || ''" :options="modeOptions" aria-label="上游模式" class="w-full" @update:model-value="set('upstreamMode', $event)" />
      </label>
      <div class="usage-filter-actions flex items-end gap-2">
        <BaseButton aria-label="更多筛选" :aria-expanded="expanded" :aria-controls="moreId" class="px-3" @click="expanded = !expanded">
          <SlidersHorizontal :size="15" />{{ expanded ? '收起筛选' : '更多筛选' }}<span v-if="moreCount" class="rounded-cp-sm bg-cp-primary-container px-1.5 py-0.5 text-cp-xs text-cp-primary-on-container">{{ moreCount }}</span>
        </BaseButton>
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
    <div v-if="activeConditions.length" class="mt-3 flex flex-wrap gap-2" aria-label="已应用的筛选条件">
      <span v-for="field in activeConditions" :key="field.key" class="inline-flex max-w-full items-center gap-1 rounded-cp-sm border border-cp-border-secondary bg-cp-bg-container px-2 text-cp-xs text-cp-text-secondary">
        <span class="min-w-0 truncate" :title="`${field.label}：${conditionText(field)}`">{{ field.label }}：{{ conditionText(field) }}</span>
        <BaseIconButton :label="`清除${field.label}条件`" size="sm" @click="set(field.key, '')"><X :size="12" /></BaseIconButton>
      </span>
    </div>
    <div v-if="retiredConditions.length" class="mt-3 flex flex-wrap gap-2" aria-label="已应用的钻取条件">
      <span v-for="field in retiredConditions" :key="field.key" class="inline-flex max-w-full items-center gap-1 rounded-cp-sm bg-cp-primary-container px-2 text-cp-xs text-cp-primary-on-container">
        <span class="min-w-0 break-all">{{ field.label }}：{{ model[field.key] }}</span>
        <BaseIconButton :label="`移除${field.label}筛选`" size="sm" @click="set(field.key, '')"><X :size="13" /></BaseIconButton>
      </span>
    </div>
    <div v-if="expanded" :id="moreId" class="usage-more-fields mt-4 border-t border-cp-border-secondary pt-4">
      <UsageEntityFilter kind="key" :model-value="model.clientApiKeyId || ''" @update:model-value="set('clientApiKeyId', $event)" />
      <UsageEntityFilter kind="group" :model-value="model.groupId || ''" @update:model-value="set('groupId', $event)" />
      <label class="min-w-0"><span class="mb-1 block text-cp-xs text-cp-text-secondary">请求接口</span><BaseSelect class="w-full" :model-value="model.route || ''" :options="routeOptions" aria-label="请求接口" @update:model-value="set('route', $event)" /></label>
      <label v-for="field in textFields" :key="field.key" class="min-w-0">
        <span class="mb-1 block text-cp-xs text-cp-text-secondary">{{ field.label }}</span>
        <BaseInput :type="field.type || 'text'" :min="field.type === 'number' ? 0 : undefined" :model-value="model[field.key] || ''" :placeholder="field.placeholder" :aria-label="field.label" @update:model-value="set(field.key, $event)" />
      </label>
      <label class="min-w-0"><span class="mb-1 block text-cp-xs text-cp-text-secondary">{{ t('usage.error.upstreamTransport') }}</span><BaseSelect class="w-full" :model-value="model.upstreamTransport || ''" :options="transportOptions" :aria-label="t('usage.error.upstreamTransport')" @update:model-value="set('upstreamTransport', $event)" /></label>
      <label class="min-w-0"><span class="mb-1 block text-cp-xs text-cp-text-secondary">{{ t('usage.error.clientTransfer') }}</span><BaseSelect class="w-full" :model-value="model.clientTransport || ''" :options="transportOptions.filter(option => option.value !== 'unknown')" :aria-label="t('usage.error.clientTransfer')" @update:model-value="set('clientTransport', $event)" /></label>
      <label class="min-w-0"><span class="mb-1 block text-cp-xs text-cp-text-secondary">{{ t('usage.cost.cache') }}</span><BaseSelect class="w-full" :model-value="model.cacheMatch || ''" :options="cacheOptions" aria-label="缓存筛选" @update:model-value="set('cacheMatch', $event)" /></label>
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

<style scoped>
.usage-filter-fields {
  display: flex;
  flex-wrap: wrap;
  align-items: end;
  gap: 12px;
}
.usage-filter-fields > * {
  flex: 1 1 125px;
  min-width: 0;
}
.usage-filter-fields > .usage-filter-time {
  flex: 1.6 1 270px;
}
.usage-filter-fields > .usage-filter-search {
  flex: 1.4 1 190px;
}
.usage-filter-fields > .usage-filter-account {
  flex: 1.2 1 165px;
}
.usage-filter-fields > .usage-filter-actions {
  flex: 0 0 auto;
  margin-left: auto;
}
.usage-more-fields {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 180px), 1fr));
  gap: 14px 12px;
  align-items: start;
}
@media (max-width: 600px) {
  .usage-filter-fields > .usage-filter-time,
  .usage-filter-fields > .usage-filter-search {
    flex-basis: 100%;
  }
  .usage-filter-fields > * {
    flex-basis: 125px;
  }
  .usage-filter-fields > .usage-filter-account {
    flex-basis: 165px;
  }
  .usage-more-fields {
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 140px), 1fr));
  }
}
</style>
