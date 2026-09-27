<script setup lang="ts">
import { Search } from '@lucide/vue'
import { computed, onBeforeUnmount, ref, useId, watch } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'

interface Option { value: string, label: string, description?: string }
const props = defineProps<{
  label: string
  searchPlaceholder?: string
  selectedLabel?: string
  multiple?: boolean
  loadPage: (page: number, search: string, signal: AbortSignal) => Promise<{
    items: Option[]
    total: number
    totalPages: number
  }>
}>()
const selected = defineModel<string>({ default: '' })
const selectedValues = defineModel<string[]>('selectedValues', { default: () => [] })
function isSelected(value: string) {
  return props.multiple ? selectedValues.value.includes(value) : selected.value === value
}
const id = useId()
const search = ref('')
const items = ref<Option[]>([])
const selectedOption = ref<Option>()
const page = ref(0)
const total = ref(0)
const totalPages = ref(0)
const loading = ref(false)
const error = ref('')
const selectionLabel = computed(() => selectedOption.value?.label || props.selectedLabel || selected.value)
let controller: AbortController | undefined
let timer: ReturnType<typeof setTimeout> | undefined

watch(selected, (value) => {
  selectedOption.value = items.value.find(item => item.value === value)
})

async function load(nextPage: number) {
  controller?.abort()
  const request = new AbortController()
  controller = request
  loading.value = true
  error.value = ''
  try {
    const result = await props.loadPage(nextPage, search.value.trim(), request.signal)
    if (request.signal.aborted)
      return
    items.value = [...new Map([
      ...(nextPage === 1 ? [] : items.value),
      ...result.items,
    ].map(item => [item.value, item])).values()]
    page.value = nextPage
    total.value = result.total
    totalPages.value = result.totalPages
    selectedOption.value = items.value.find(item => item.value === selected.value) || selectedOption.value
  }
  catch (cause) {
    if (!request.signal.aborted)
      error.value = cause instanceof Error ? cause.message : '加载失败，请重试'
  }
  finally {
    if (!request.signal.aborted)
      loading.value = false
  }
}

watch(search, (_, previous) => {
  controller?.abort()
  clearTimeout(timer)
  items.value = []
  page.value = 0
  total.value = 0
  totalPages.value = 0
  error.value = ''
  loading.value = true
  if (previous === undefined)
    void load(1)
  else
    timer = setTimeout(() => void load(1), 250)
}, { immediate: true, flush: 'sync' })

onBeforeUnmount(() => {
  controller?.abort()
  clearTimeout(timer)
})
</script>

<template>
  <fieldset class="min-w-0">
    <legend class="mb-2 text-cp-sm font-medium">
      {{ label }} <span class="text-cp-error">*</span>
    </legend>
    <div class="min-w-0 overflow-hidden rounded-cp border border-cp-border">
      <div class="border-b border-cp-border p-2">
        <BaseInput v-model="search" :aria-label="`搜索${label}`" :placeholder="searchPlaceholder || '搜索名称'" size="sm">
          <template #prefix>
            <Search class="size-4" />
          </template>
        </BaseInput>
      </div>
      <div class="max-h-52 min-h-24 overflow-y-auto overscroll-contain" :aria-label="`${label}列表`" :aria-busy="loading">
        <label
          v-for="item in items" :key="item.value"
          :for="`${id}-${item.value}`"
          class="flex min-w-0 cursor-pointer items-start gap-3 border-b border-cp-border px-3 py-2.5 text-cp-sm last:border-b-0 hover:bg-cp-fill-tertiary"
          :class="isSelected(item.value) ? 'bg-cp-fill-tertiary' : ''"
        >
          <input v-if="multiple" :id="`${id}-${item.value}`" v-model="selectedValues" type="checkbox" :value="item.value" :aria-label="item.label" class="mt-1 shrink-0 accent-[var(--cp-color-primary)]">
          <input v-else :id="`${id}-${item.value}`" v-model="selected" type="radio" :name="id" :value="item.value" :aria-label="item.label" class="mt-1 shrink-0 accent-[var(--cp-color-primary)]">
          <span class="min-w-0">
            <span class="block break-words [overflow-wrap:anywhere]">{{ item.label }}</span>
            <span v-if="item.description" class="mt-0.5 block break-words text-xs text-cp-text-secondary [overflow-wrap:anywhere]">{{ item.description }}</span>
          </span>
        </label>
        <p v-if="loading" role="status" class="px-3 py-4 text-center text-cp-sm text-cp-text-secondary">
          加载中…
        </p>
        <div v-else-if="error" class="grid justify-items-center gap-2 px-3 py-4">
          <p role="alert" class="break-words text-cp-sm text-cp-error [overflow-wrap:anywhere]">
            {{ error }}
          </p>
          <BaseButton type="button" @click="load(page + 1)">
            重试加载{{ label }}
          </BaseButton>
        </div>
        <p v-else-if="!items.length" class="px-3 py-4 text-center text-cp-sm text-cp-text-secondary">
          {{ search.trim() ? '没有匹配结果' : `暂无可选${label}` }}
        </p>
        <div v-else-if="page < totalPages" class="p-2 text-center">
          <BaseButton type="button" @click="load(page + 1)">
            加载更多{{ label }}
          </BaseButton>
        </div>
      </div>
      <div class="flex min-w-0 flex-wrap justify-between gap-x-3 gap-y-1 border-t border-cp-border px-3 py-2 text-xs text-cp-text-secondary" aria-live="polite">
        <span class="min-w-0 break-words [overflow-wrap:anywhere]">{{ multiple ? `已选 ${selectedValues.length} 项` : selected ? `已选：${selectionLabel}` : '未选择' }}</span>
        <span v-if="page" class="shrink-0">{{ items.length }} / {{ total }}</span>
      </div>
    </div>
  </fieldset>
</template>
