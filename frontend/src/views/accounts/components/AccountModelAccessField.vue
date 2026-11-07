<script setup lang="ts">
import type { AccountModelAccess } from '@/api'
import { CircleAlert, ListChecks, ListMinus, Plus, RefreshCw, Search } from '@lucide/vue'

import { computed, ref, useId, useTemplateRef, watch } from 'vue'
import { getAccountModels, refreshAccountModels } from '@/api'
import { getQualityModels } from '@/api/modules/quality-ops'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseEmpty from '@/components/base/BaseEmpty.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BasePopover from '@/components/base/BasePopover.vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'
import { useRequestState } from '@/composables/useRequestState'
import { t } from '@/i18n'
import { accountModelAccessError, accountModelIdError } from '../utils/modelAccess'
import AccountModelCleanupPopover from './AccountModelCleanupPopover.vue'
import AccountModelOption from './AccountModelOption.vue'

const props = withDefaults(defineProps<{
  accountId?: string
  formProp?: string
  knownModelCatalog?: boolean
  disabled?: boolean
  allowPreserve?: boolean
}>(), { disabled: false, allowPreserve: false, knownModelCatalog: false })
const model = defineModel<AccountModelAccess | undefined>({ required: true })
const formItem = useTemplateRef<InstanceType<typeof BaseFormItem>>('modelAccessItem')
watch(model, () => formItem.value?.clearValidate())
const catalog = ref<Array<{ id: string, label: string }>>([])
const refreshButton = ref<InstanceType<typeof BaseIconButton>>()
const catalogObservedAtDisplay = ref<string>()
const catalogHelpOpen = ref(false)
const catalogHelpId = useId()
const nextPage = ref<number | null>(1)
const partialCatalog = ref(false)
const catalogLoaded = ref(false)
const retryAppend = ref(false)
const useKnownCatalog = computed(() => !props.accountId && props.knownModelCatalog)
const search = ref('')
const selectionError = ref('')
const searchInput = useTemplateRef<InstanceType<typeof BaseInput>>('searchInput')
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
    selectionError.value = ''
    if (value === 'preserve') {
      model.value = undefined
      return
    }
    model.value = { mode: value as AccountModelAccess['mode'], models: value === 'all' ? [] : [...(model.value?.models ?? [])] }
  },
})
const restricted = computed(() => mode.value === 'allowlist' || mode.value === 'denylist')
const selectedIds = computed(() => new Set(model.value?.models))
// 候选项保留到本次编辑结束，取消勾选只修改待保存的模型限制。
const availableIds = ref<string[]>([])
const catalogReady = computed(() => canIdentifyMissing.value)
const extraIds = computed(() => {
  const ids = new Set(catalog.value?.map(item => item.id))
  return availableIds.value.filter(id => !ids.has(id))
})
// 目录已就绪且非空时才能按目录清理白名单
const missingModels = computed(() => mode.value === 'allowlist' && catalogReady.value && catalog.value?.length
  ? extraIds.value.filter(id => selectedIds.value.has(id))
  : [])
const query = computed(() => search.value.trim().toLowerCase())
const catalogResults = computed(() => (catalog.value ?? [])
  .filter(item => `${item.id} ${item.label}`.toLowerCase().includes(query.value))
  .map(item => item.id))
const allResultsSelected = computed(() => catalogResults.value.every(id => selectedIds.value.has(id)))
const groups = computed(() => [
  {
    key: 'selected',
    label: catalogReady.value
      ? t('accounts.modelAccess.customModels')
      : t('accounts.modelAccess.availableModels'),
    ids: extraIds.value.filter(id => id.toLowerCase().includes(query.value)),
  },
  {
    key: 'catalog',
    label: t('accounts.modelAccess.upstreamCatalog'),
    ids: catalogResults.value,
  },
].filter(group => group.ids.length))
const newModelId = computed(() => groups.value.length
  ? ''
  : search.value.trim())
const inputError = computed(() => selectionError.value || (newModelId.value && accountModelIdError(newModelId.value)))
const canAdd = computed(() => Boolean(newModelId.value) && !inputError.value)

function removeMissingModels(ids: string[]) {
  if (props.disabled || model.value?.mode !== 'allowlist')
    return
  const selected = new Set(ids)
  const removed = new Set(missingModels.value.filter(id => selected.has(id)))
  if (!removed.size)
    return
  const next = {
    ...model.value,
    models: model.value.models.filter(id => !removed.has(id)),
  }
  selectionError.value = accountModelAccessError(next) ?? ''
  if (!selectionError.value) {
    availableIds.value = availableIds.value.filter(id => !removed.has(id))
    model.value = next
  }
}

function restoreCleanupFocus() {
  // 最后一项清理后入口禁用，改回相邻刷新按钮，避免焦点落到页面背景
  if (!missingModels.value.length && document.activeElement === document.body)
    (refreshButton.value?.$el as HTMLElement | undefined)?.focus()
}

function select(modelIds: string[], selected: boolean) {
  if (props.disabled || !model.value || !restricted.value)
    return
  const ids = new Set(selectedIds.value)
  for (const id of modelIds) {
    if (selected)
      ids.add(id)
    else ids.delete(id)
  }
  const next = {
    ...model.value,
    models: [...ids],
  }
  selectionError.value = selected
    ? accountModelAccessError(next) ?? ''
    : ''
  if (!selectionError.value)
    model.value = next
}

function addModel() {
  if (props.disabled || !restricted.value || !canAdd.value)
    return
  select([newModelId.value], true)
  if (!selectionError.value) {
    search.value = ''
    const input = (searchInput.value?.$el as HTMLElement | undefined)?.querySelector('input')
    input?.focus()
  }
}

function handleSearchEnter(event: KeyboardEvent) {
  if (event.isComposing)
    return
  event.preventDefault()
  addModel()
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

watch([
  () => props.accountId,
  () => model.value?.models,
], ([accountId, ids], [previousAccountId]) => {
  const retained = accountId === previousAccountId
    ? availableIds.value
    : []
  availableIds.value = [
    ...new Set([
      ...retained,
      ...(ids ?? []),
    ]),
  ]
}, { immediate: true })

watch([() => props.accountId, () => props.knownModelCatalog, restricted, () => props.disabled], () => {
  request.invalidate()
  catalog.value = []
  catalogObservedAtDisplay.value = undefined
  catalogHelpOpen.value = false
  nextPage.value = 1
  partialCatalog.value = false
  catalogLoaded.value = false
  search.value = ''
  selectionError.value = ''
  error.value = ''
  void load()
}, { immediate: true })

watch(search, () => {
  selectionError.value = ''
})
</script>

<template>
  <div class="@container grid gap-3">
    <BaseFormItem ref="modelAccessItem" :prop="formProp">
      <template #label>
        <span class="inline-flex flex-wrap items-end gap-x-2 gap-y-1">
          <span>{{ t('accounts.modelAccess.modelRestrictionMode') }}</span>
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
        <AccountModelCleanupPopover v-if="mode === 'allowlist'" :models="missingModels" :model-count="model?.models.length ?? 0" :disabled="disabled || !missingModels.length" @remove="removeMissingModels" @closed="restoreCleanupFocus" />
        <BaseIconButton v-if="restricted && (accountId || useKnownCatalog)" ref="refreshButton" :label="t('accounts.modelAccess.refreshModel')" size="sm" :loading="loading" :disabled="disabled" @click="load(true)">
          <template #loading>
            <RefreshCw class="size-3.5 animate-spin motion-reduce:animate-none" />
          </template>
          <RefreshCw class="size-3.5" />
        </BaseIconButton>
      </template>
      <BaseSegmented v-model="mode" class="w-full" :label="t('accounts.modelAccess.modelRestrictionMode')" :options="modes" :disabled="disabled" />
    </BaseFormItem>
    <template v-if="restricted">
      <div class="flex min-w-0 items-center gap-2">
        <BaseInput ref="searchInput" v-model="search" class="min-w-0 flex-1" :aria-label="t('accounts.modelAccess.searchOrAddModels')" :placeholder="t('accounts.modelAccess.searchOrEnterId')" :disabled="disabled" :aria-invalid="Boolean(inputError)" @keydown.enter="handleSearchEnter">
          <template #prefix>
            <Search class="size-4" aria-hidden="true" />
          </template>
        </BaseInput>
        <BaseButton class="shrink-0" :disabled="disabled || !canAdd" @click="addModel">
          <Plus class="size-4" aria-hidden="true" />
          {{ t('accounts.modelAccess.addModel') }}
        </BaseButton>
      </div>
      <p v-if="inputError" class="m-0 text-cp-xs text-cp-error-text" role="alert">
        {{ inputError }}
      </p>
      <div v-if="groups.length" class="grid min-w-0 gap-3" :aria-busy="loading || undefined">
        <div v-for="group in groups" :key="group.key" role="group" :aria-label="group.label">
          <div class="mb-1.5 flex items-center justify-between gap-2">
            <span class="text-cp-xs font-medium text-cp-text-secondary">{{ group.label }}</span>
            <BaseIconButton
              v-if="group.key === 'catalog'"
              variant="ghost"
              size="sm"
              class="-my-1 size-6!"
              :label="allResultsSelected ? t('accounts.modelAccess.deselectAll') : t('accounts.modelAccess.selectAll')"
              :disabled="disabled || loading"
              @click="select(group.ids, !allResultsSelected)"
            >
              <ListMinus v-if="allResultsSelected" aria-hidden="true" />
              <ListChecks v-else aria-hidden="true" />
            </BaseIconButton>
          </div>
          <div class="grid grid-cols-2 gap-2 @xl:grid-cols-3 @3xl:grid-cols-4">
            <div
              v-for="id in group.ids"
              :key="id"
              class="flex min-w-0 items-center rounded-cp transition-colors duration-150 motion-reduce:transition-none"
              :class="selectedIds.has(id) ? 'bg-cp-primary-container text-cp-primary-on-container' : 'bg-cp-fill-quaternary text-cp-text-secondary hover:bg-cp-fill-tertiary hover:text-cp-text'"
            >
              <AccountModelOption :model-value="selectedIds.has(id)" :label="id" :disabled="disabled" @update:model-value="select([id], $event)" />
            </div>
          </div>
        </div>
      </div>
      <p v-else-if="loading" class="m-0 py-4 text-center text-cp-sm text-cp-text-tertiary" role="status">
        {{ t('accounts.modelAccess.loadingModel') }}
      </p>
      <p v-else-if="mode === 'denylist' && canAdd" class="m-0 text-cp-xs text-cp-text-secondary">
        {{ t('accounts.modelAccess.addToDenylistHint') }}
      </p>
      <BaseEmpty v-else-if="!error && !search" :title="t('accounts.modelAccess.noModels')" size="sm" surface="none" />
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
