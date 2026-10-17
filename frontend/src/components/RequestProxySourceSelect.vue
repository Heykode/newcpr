<script setup lang="ts">
import type { RequestProxySource } from '@/utils/request-proxy-source'
import { computed } from 'vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { requestProxySources } from '@/utils/request-proxy-source'

defineProps<{ disabled?: boolean }>()
const model = defineModel<RequestProxySource>({ required: true })
const options = computed(() => [
  ...(model.value === 'proxy_pool' ? [{ value: 'proxy_pool', label: '普通代理池（原设置）', disabled: true }] : []),
  ...requestProxySources,
])
function update(value: string) {
  if (value === 'account' || value === 'mihomo')
    model.value = value
}
</script>

<template>
  <BaseSelect :model-value="model" :options="options" :disabled="disabled" aria-label="账号请求出口" @update:model-value="update" />
</template>
