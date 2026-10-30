<script setup lang="ts">
import type { AccountModelAccess } from '@/api'
import { CircleAlert, RefreshCw, Search } from '@lucide/vue'

import { computed, ref, useId, watch } from 'vue'
import { getAccountModels, refreshAccountModels } from '@/api'
import { getQualityModels } from '@/api/modules/quality-ops'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseEmpty from '@/components/base/BaseEmpty.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BasePopover from '@/components/base/BasePopover.vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'
import { useRequestState } from '@/composables/useRequestState'
import { t } from '@/i18n'
import { accountModelAccessError, accountModelIdError } from '../utils/modelAccess'

const props = withDefaults(defineProps<{
  accountId?: string
  knownModelCatalog?: boolean
  disabled?: boolean
  allowPreserve?: boolean
}>(), { disabled: false, allowPreserve: false, knownModelCatalog: false })
const model = defineModel<AccountModelAccess | undefined>({ required: true })
const catalog = ref<Array<{ id: string, label: string }>>([])
const catalogObservedAtDisplay = ref<string>()
const catalogHelpOpen = ref(false)
const catalogHelpId = useId()
const nextPage = ref<number | null>(1)
const partialCatalog = ref(false)
const catalogLoaded = ref(false)
const retryAppend = ref(false)
const useKnownCatalog = computed(() => !props.accountId && props.knownModelCatalog)
const search = ref('')
const inputError = ref('')
const request = useRequestState()
const { loading, error } = request
const canIdentifyMissing = computed(() => catalogLoaded.value && !loading.value && !error.value && !partialCatalog.value && (!useKnownCatalog.value || nextPage.value === null))
const modes = computed(() => [
  ...(props.allowPreserve ? [{ value: 'preserve', label: '保留' }] : []),
  { value: 'all', label: '不限制' },
  { value: 'allowlist', label: '白名单' },
  { value: 'denylist', label: '黑名单' },
])
const mode = computed({
  get: () => model.value?.mode ?? (props.allowPreserve ? 'preserve' : 'all'),
  set: (value: string) => {
    inputError.value = ''
    if (value === 'preserve') {
      model.value = undefined
      return
    }
    model.value = { mode: value as AccountModelAccess['mode'], models: value === 'all' ? [] : [...(model.value?.models ?? [])] }
  },
})
const restricted = computed(() => mode.value === 'allowlist' || mode.value === 'denylist')
const canAdd = computed(() => Boolean(search.value.trim()) && !model.value?.models.includes(search.value.trim()))
const models = computed(() => {
  const entries = new Map(catalog.value.map(item => [item.id, { ...item, unavailable: false }]))
  for (const id of model.value?.models ?? []) {
    if (!entries.has(id))
      entries.set(id, { id, label: id, unavailable: canIdentifyMissing.value })
  }
  const query = search.value.trim().toLowerCase()
  return [...entries.values()].filter(item => `${item.id} ${item.label}`.toLowerCase().includes(query))
})

function select(id: string, selected: boolean) {
  if (props.disabled || !model.value || !restricted.value)
    return
  const ids = new Set(model.value.models)
  if (selected)
    ids.add(id)
  else ids.delete(id)
  const next = { ...model.value, models: [...ids] }
  inputError.value = selected ? accountModelAccessError(next) ?? '' : ''
  if (!inputError.value)
    model.value = next
}

function addModel() {
  if (props.disabled || !restricted.value || !canAdd.value)
    return
  const id = search.value.trim()
  inputError.value = accountModelIdError(id) ?? ''
  if (inputError.value)
    return
  select(id, true)
  if (!inputError.value)
    search.value = ''
}

function handleCatalogHelpKeydown(event: KeyboardEvent, close: () => void) {
  if (event.key === 'Escape' && catalogHelpOpen.value) {
    // 先关闭说明浮层，避免同一次按键也关闭编辑弹窗。
    event.preventDefault()
    event.stopPropagation()
    close()
  }
}

async function load(refresh = false, append = false) {
  const accountId = props.accountId
  if ((!accountId && !useKnownCatalog.value) || !restricted.value || props.disabled || loading.value)
    return
  const page = append ? nextPage.value : 1
  if (page === null)
    return
  const requestId = request.start()
  try {
    if (useKnownCatalog.value) {
      const result = await getQualityModels({}, page, { signal: request.signal, silent: true })
      if (request.isCurrent(requestId)) {
        const entries = new Map((append ? catalog.value : []).map(item => [item.id, item]))
        for (const item of result.models)
          entries.set(item.id, { id: item.id, label: item.name })
        catalog.value = [...entries.values()]
        nextPage.value = result.nextPage
        partialCatalog.value = (append && partialCatalog.value) || result.failedAccounts > 0
        catalogLoaded.value = true
      }
    }
    else if (accountId) {
      const result = await (refresh ? refreshAccountModels : getAccountModels)(
        { accountId },
        { signal: request.signal, silent: true },
      )
      if (request.isCurrent(requestId)) {
        catalog.value = result.models
        catalogObservedAtDisplay.value = result.observedAt ? result.observedAtDisplay : undefined
        catalogLoaded.value = true
      }
    }
  }
  catch (cause) {
    if (request.isCurrent(requestId))
      retryAppend.value = append
    request.fail(requestId, cause)
  }
  finally {
    request.finish(requestId)
  }
}

watch([() => props.accountId, () => props.knownModelCatalog, restricted, () => props.disabled], () => {
  request.invalidate()
  catalog.value = []
  catalogObservedAtDisplay.value = undefined
  catalogHelpOpen.value = false
  nextPage.value = 1
  partialCatalog.value = false
  catalogLoaded.value = false
  search.value = ''
  inputError.value = ''
  error.value = ''
  void load()
}, { immediate: true })

watch(search, () => {
  inputError.value = ''
})
</script>

<template>
  <div class="grid gap-3">
    <BaseFormItem>
      <template #label>
        <span class="inline-flex flex-wrap items-end gap-x-2 gap-y-1">
          <span>{{ t('accounts.modelAccess.modelCatalog') }}</span>
          <span v-if="restricted && catalogObservedAtDisplay" class="text-cp-xs leading-none font-normal text-cp-text-tertiary tabular-nums">
            {{ catalogObservedAtDisplay }}
          </span>
        </span>
      </template>
      <template #extra>
        <BasePopover v-if="restricted" v-model="catalogHelpOpen" trigger="hover-click" placement="top-start">
          <template #trigger="{ close }">
            <BaseIconButton
              :label="t('accounts.modelAccess.catalogHelpTitle')"
              size="sm"
              :show-tooltip="false"
              :aria-describedby="catalogHelpOpen ? catalogHelpId : undefined"
              :aria-expanded="catalogHelpOpen"
              @keydown="handleCatalogHelpKeydown($event, close)"
            >
              <CircleAlert class="size-3.5" aria-hidden="true" />
            </BaseIconButton>
          </template>
          <div :id="catalogHelpId" role="tooltip" class="max-w-72 space-y-2 px-3 py-2 text-cp-sm leading-relaxed text-cp-text-secondary">
            <p class="m-0">
              {{ t('accounts.modelAccess.catalogObservedAtHelp') }}
            </p>
            <p class="m-0">
              {{ t('accounts.modelAccess.catalogDescription') }}
            </p>
          </div>
        </BasePopover>
        <slot name="extra" />
        <BaseIconButton v-if="restricted && (accountId || useKnownCatalog)" :label="t('accounts.modelAccess.refreshModel')" size="sm" :loading="loading" :disabled="disabled" @click="load(true)">
          <template #loading>
            <RefreshCw class="size-3.5 animate-spin motion-reduce:animate-none" />
          </template>
          <RefreshCw class="size-3.5" />
        </BaseIconButton>
      </template>
      <BaseSegmented v-model="mode" class="w-full" :label="t('accounts.modelAccess.modelRestrictionMode')" :options="modes" :disabled="disabled" />
    </BaseFormItem>
    <template v-if="restricted">
      <BaseInput v-model="search" :aria-label="t('accounts.modelAccess.searchOrAddModels')" :placeholder="t('accounts.modelAccess.searchOrEnterIdAndPressEnter')" :disabled="disabled" :aria-invalid="Boolean(inputError)" @keydown.enter.prevent="addModel">
        <template #prefix>
          <Search class="size-4" aria-hidden="true" />
        </template>
      </BaseInput>
      <p v-if="useKnownCatalog" class="m-0 text-cp-xs text-cp-text-tertiary">
        可勾选已有账号的已知模型，也可输入完整模型 ID 后按回车添加。列表不代表每个账号都支持所有模型。
      </p>
      <p v-if="inputError" class="m-0 text-cp-xs text-cp-error-text" role="alert">
        {{ inputError }}
      </p>
      <div v-if="models.length" class="grid max-h-60 grid-cols-2 gap-2 overflow-y-auto p-1 -m-1 sm:grid-cols-3" role="group" :aria-label="t('accounts.modelAccess.selectModel')" :aria-busy="loading || undefined">
        <BasePopover v-for="item in models" :key="item.id" trigger="hover-click" class="min-w-0">
          <template #trigger>
            <BaseCheckbox
              class="min-h-11 w-full min-w-0 rounded-cp px-3 py-2.5 wrap-anywhere"
              :class="item.unavailable ? 'bg-cp-warning-container text-cp-warning' : model?.models.includes(item.id) ? 'bg-cp-primary-container text-cp-primary-on-container' : 'bg-cp-fill-quaternary text-cp-text-secondary'"
              :model-value="model?.models.includes(item.id) ?? false"
              :label="item.id"
              :title="item.unavailable ? '当前目录未返回，仍可保存' : item.id"
              show-label
              :disabled="disabled"
              @update:model-value="select(item.id, $event)"
            />
          </template>
          <div class="max-w-80 break-all p-3 text-cp-sm">
            {{ item.id }}
            <p v-if="item.unavailable" class="mb-0 text-cp-warning">
              当前目录未返回，仍可保存
            </p>
          </div>
        </BasePopover>
      </div>
      <p v-else-if="loading" class="m-0 py-4 text-center text-cp-sm text-cp-text-tertiary" role="status">
        加载模型中…
      </p>
      <BaseEmpty v-else-if="!error" :title="search ? '无匹配模型' : '暂无模型'" :icon="search ? Search : undefined" size="sm" surface="none" />
      <p v-if="error" class="m-0 text-cp-xs text-cp-error-text" role="alert">
        模型列表加载失败，已选模型仍保留，可重试或手动输入。
      </p>
      <p v-if="useKnownCatalog && partialCatalog" class="m-0 text-cp-xs text-cp-text-tertiary" role="status">
        部分账号的模型目录暂不可用，已显示读取成功的模型，仍可手动添加。
      </p>
      <BaseButton v-if="useKnownCatalog && (nextPage !== null || error)" :loading="loading" :disabled="disabled" @click="load(false, error ? retryAppend : true)">
        {{ error ? '重试加载模型' : '加载更多模型' }}
      </BaseButton>
    </template>
  </div>
</template>
