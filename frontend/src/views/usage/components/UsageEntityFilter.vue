<script setup lang="ts">
import { X } from '@lucide/vue'
import { onClickOutside, watchDebounced } from '@vueuse/core'
import { computed, onScopeDispose, ref, shallowRef, watch } from 'vue'
import { getAccountGroups, getApiKeys } from '@/api'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'

const props = defineProps<{ kind: 'key' | 'group' }>()
const model = defineModel<string>({ default: '' })
const label = computed(() => props.kind === 'key' ? 'API Key' : '请求分组')
const root = ref<HTMLElement>()
const query = ref('')
const open = ref(false)
const loading = ref(false)
const failed = ref(false)
const items = shallowRef<{ id: string, name: string, detail: string }[]>([])
const names = shallowRef<Record<string, string>>({})
let controller: AbortController | undefined
let generation = 0

function close() {
  open.value = false
  ++generation
  controller?.abort()
  loading.value = false
}
async function load() {
  const current = ++generation
  controller?.abort()
  controller = new AbortController()
  loading.value = true
  failed.value = false
  try {
    const search = query.value.trim() || undefined
    const options = { signal: controller.signal }
    const result = props.kind === 'key'
      ? (await getApiKeys({ limit: 50, search }, options)).items.map(item => ({
          id: item.id,
          name: item.name,
          detail: item.prefix,
        }))
      : (await getAccountGroups({ page: 1, pageSize: 50, search }, options)).items.map(item => ({
          id: item.id,
          name: item.name,
          detail: item.id,
        }))
    if (current !== generation)
      return
    items.value = result
    names.value = { ...names.value, ...Object.fromEntries(result.map(item => [item.id, item.name])) }
  }
  catch {
    if (current === generation)
      failed.value = true
  }
  finally {
    if (current === generation)
      loading.value = false
  }
}
function select(id: string) {
  model.value = id
  close()
}
watchDebounced(query, () => {
  if (open.value)
    void load()
}, { debounce: 250 })
watch(open, (value) => {
  if (value)
    void load()
})
watch(model, (value) => {
  if (!value)
    query.value = ''
})
onClickOutside(root, close)
onScopeDispose(close)
</script>

<template>
  <!-- eslint-disable vue-a11y/label-has-for -- BaseInput renders the associated input within this label. -->
  <div ref="root" class="relative min-w-0">
    <label>
      <span class="mb-1 block text-cp-xs text-cp-text-secondary">{{ label }}</span>
      <BaseInput v-model="query" :aria-label="`搜索${label}`" :placeholder="model ? (names[model] || model) : `搜索${label}`" @focusin="open = true" @keydown.esc="close" />
    </label>
    <div v-if="model" class="mt-1 flex min-w-0 items-center gap-1 text-cp-xs">
      <span class="min-w-0 flex-1 break-all">{{ names[model] || model }}</span>
      <BaseIconButton :label="`移除${label}筛选`" size="sm" @click="model = ''; query = ''">
        <X :size="13" />
      </BaseIconButton>
    </div>
    <div v-if="open" class="absolute right-0 left-0 z-40 mt-1 max-h-64 overflow-auto rounded-cp border border-cp-border bg-cp-bg-elevated p-1 shadow-cp" :aria-label="`${label}候选`">
      <p v-if="loading" class="px-2 text-cp-xs">
        加载中
      </p>
      <p v-else-if="failed" role="alert" class="px-2 text-cp-xs text-cp-error-text">
        查询失败
      </p>
      <p v-else-if="!items.length" class="px-2 text-cp-xs text-cp-text-secondary">
        暂无匹配项
      </p>
      <button v-for="item in items" :key="item.id" type="button" class="block w-full cursor-pointer rounded-cp-sm border-0 bg-transparent p-2 text-left hover:bg-cp-bg-text-hover" @click="select(item.id)" @keydown.esc="close">
        <span class="block break-all text-cp-sm text-cp-text">{{ item.name }}</span>
        <span class="block break-all text-cp-xs text-cp-text-secondary">{{ item.detail }}</span>
      </button>
      <button v-if="query.trim()" type="button" class="block w-full cursor-pointer border-0 bg-transparent p-2 text-left text-cp-xs text-cp-text-secondary hover:bg-cp-bg-text-hover" @click="select(query.trim())">
        按精确 ID：{{ query.trim() }}
      </button>
    </div>
  </div>
</template>
