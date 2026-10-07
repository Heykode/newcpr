<script setup lang="ts">
import type { QualityModelChoice, QualityModelScope } from '@/api/modules/quality-ops'
import { ChevronDown, LoaderCircle, RefreshCw } from '@lucide/vue'
import { computed, nextTick, onBeforeUnmount, ref, useId, watch } from 'vue'
import { getQualityModels } from '@/api/modules/quality-ops'
import BaseInput from '@/components/base/BaseInput.vue'
import { mergeModelChoices } from './model-choices'

const props = defineProps<{ scope: QualityModelScope, disabled?: boolean }>()
const emit = defineEmits<{ choices: [value: QualityModelChoice[]] }>()
const model = defineModel<string>({ default: '' })
const root = ref<HTMLElement>()
const open = ref(false)
const query = ref('')
const choices = ref<QualityModelChoice[]>([])
const loading = ref(false)
const error = ref('')
const nextPage = ref<number | null>(1)
const active = ref(-1)
const counts = ref({ matched: 0, known: 0, failed: 0 })
const listId = useId()
const filtered = computed(() => choices.value.filter(choice => `${choice.id} ${choice.name}`.toLowerCase().includes(query.value.toLowerCase())))
let controller: AbortController | undefined
let generation = 0

function cancel() {
  generation++
  controller?.abort()
  loading.value = false
}
function close() {
  open.value = false
  active.value = -1
  cancel()
}
async function load() {
  if (loading.value || nextPage.value === null || !open.value)
    return
  const token = ++generation
  controller = new AbortController()
  loading.value = true
  error.value = ''
  try {
    const page = await getQualityModels(props.scope, nextPage.value, { signal: controller.signal, silent: true })
    if (token !== generation)
      return
    choices.value = mergeModelChoices(choices.value, page.models)
    nextPage.value = page.nextPage
    counts.value.matched += page.matchedAccounts
    counts.value.known += page.knownAccounts
    counts.value.failed += page.failedAccounts
    emit('choices', choices.value)
  }
  catch {
    if (token === generation)
      error.value = '目录读取失败'
  }
  finally {
    if (token === generation)
      loading.value = false
  }
}
function expand() {
  if (props.disabled || open.value)
    return
  open.value = true
  query.value = ''
  if (nextPage.value === 1)
    void load()
}
function select(choice: QualityModelChoice) {
  model.value = choice.id
  close()
}
async function keydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    if (open.value) {
      event.preventDefault()
      event.stopPropagation()
      close()
    }
  }
  else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault()
    expand()
    const length = filtered.value.length
    if (length)
      active.value = (active.value + (event.key === 'ArrowDown' ? 1 : length - 1) + length) % length
    await nextTick()
    root.value?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' })
  }
  else if (event.key === 'Enter' && open.value) {
    event.preventDefault()
    const choice = filtered.value[active.value]
    if (choice)
      select(choice)
    else close()
  }
}
function blur(event: FocusEvent) {
  if (!root.value?.contains(event.relatedTarget as Node | null))
    close()
}
function reset() {
  cancel()
  choices.value = []
  emit('choices', [])
  nextPage.value = 1
  counts.value = { matched: 0, known: 0, failed: 0 }
  error.value = ''
  active.value = -1
  if (open.value)
    void load()
}
function loadNextPage() {
  // Loading replaces the action buttons; keep focus inside the stable input.
  root.value?.querySelector('input')?.focus({ preventScroll: true })
  void load()
}
function refresh() {
  root.value?.querySelector('input')?.focus({ preventScroll: true })
  reset()
}
watch(() => JSON.stringify(props.scope), reset)
watch(filtered, () => {
  active.value = -1
})
function input(value: string) {
  expand()
  query.value = value
}
onBeforeUnmount(cancel)
</script>

<template>
  <div ref="root" class="relative min-w-0" @focusout="blur">
    <BaseInput
      v-model="model" placeholder="模型 ID" :disabled="disabled" autocomplete="off"
      role="combobox" aria-autocomplete="list" :aria-expanded="open" :aria-controls="listId"
      :aria-activedescendant="active >= 0 ? `${listId}-${active}` : undefined"
      @focus="expand" @click="expand" @keydown="keydown"
      @update:model-value="input"
    >
      <template #suffix>
        <button type="button" class="flex size-6 shrink-0 items-center justify-center" aria-label="模型候选" title="模型候选" :disabled="disabled" @mousedown.prevent @click="open ? close() : expand()">
          <ChevronDown class="size-4" />
        </button>
      </template>
    </BaseInput>
    <div v-if="open" class="absolute inset-x-0 top-full z-30 mt-1 min-w-0 rounded border border-cp-border bg-cp-bg-container p-1 shadow-lg">
      <ul :id="listId" role="listbox" aria-label="模型候选" class="max-h-52 overflow-y-auto overscroll-contain">
        <li v-for="(choice, index) in filtered" :id="`${listId}-${index}`" :key="choice.id" role="option" tabindex="-1" :aria-selected="index === active" class="cursor-pointer break-all rounded px-2 py-2 text-cp-sm hover:bg-cp-fill" :class="index === active && 'bg-cp-fill'" @mousedown.prevent @click="select(choice)" @keydown.enter.prevent="select(choice)">
          {{ choice.id }}
        </li>
      </ul>
      <div class="flex min-w-0 flex-wrap items-center gap-2 border-t border-cp-border px-2 py-2 text-xs text-cp-text-secondary" aria-live="polite">
        <LoaderCircle v-if="loading" class="size-3 animate-spin" />
        <span v-if="error" role="alert">{{ error }}</span>
        <span v-else-if="!loading && !choices.length">暂无已知模型</span>
        <span v-else-if="counts.matched > 1">目录覆盖 {{ counts.known }}/{{ counts.matched }} 个账号</span>
        <span v-if="counts.failed">{{ counts.failed }} 个目录读取失败</span>
        <button v-if="!loading && (nextPage !== 1 || choices.length)" type="button" class="flex size-6 items-center justify-center" aria-label="刷新模型目录" title="刷新模型目录" @mousedown.prevent @click="refresh">
          <RefreshCw class="size-3" />
        </button>
        <button v-if="nextPage !== null && !loading" type="button" class="inline-flex items-center gap-1 text-cp-primary" @mousedown.prevent @click="loadNextPage">
          <RefreshCw v-if="error" class="size-3" />{{ error ? '重试' : '加载更多' }}
        </button>
      </div>
    </div>
  </div>
</template>
