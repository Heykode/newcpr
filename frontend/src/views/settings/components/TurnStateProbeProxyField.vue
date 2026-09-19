<script setup lang="ts">
import { RefreshCw } from '@lucide/vue'
import { computed } from 'vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { useProxyCatalog } from '@/composables/useProxyCatalog'

defineProps<{ disabled?: boolean }>()
const selection = defineModel<string>({ required: true })
const { proxies, loading, loadProxies } = useProxyCatalog()
const options = computed(() => [
  { label: 'IPv6 池', value: '' },
  ...(selection.value && !proxies.value.some(proxy => proxy.id === selection.value)
    ? [{ label: '已保存代理（目录未加载）', value: selection.value, disabled: true }]
    : []),
  ...proxies.value.map(proxy => ({
    label: `${proxy.name}${proxy.lastTest?.success ? '' : '（未通过测试）'}`,
    value: proxy.id,
    disabled: proxy.lastTest?.success !== true,
  })),
])
</script>

<template>
  <BaseFormItem label="State 探测出口">
    <div class="flex min-w-0 items-center gap-2">
      <BaseSelect
        v-model="selection"
        class="min-w-0 flex-1"
        :options="options"
        :disabled="disabled || loading"
        aria-label="State 探测出口"
      />
      <button
        type="button"
        class="flex size-9 shrink-0 items-center justify-center rounded border border-(--cp-border-color) text-cp-text-secondary disabled:opacity-50"
        title="刷新代理列表"
        aria-label="刷新代理列表"
        :disabled="disabled || loading"
        @click="loadProxies"
      >
        <RefreshCw class="size-4" :class="{ 'animate-spin': loading }" />
      </button>
    </div>
  </BaseFormItem>
</template>
