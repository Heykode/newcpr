<script setup lang="ts">
import type { RequestProxySource } from '@/utils/request-proxy-source'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { requestProxySources } from '@/utils/request-proxy-source'

defineProps<{ disabled?: boolean }>()
const model = defineModel<RequestProxySource>({ required: true })
function update(value: string) {
  if (value === 'account' || value === 'mihomo' || value === 'proxy_pool')
    model.value = value
}
</script>

<template>
  <BaseSelect :model-value="model" :options="requestProxySources" :disabled="disabled" aria-label="账号请求出口" @update:model-value="update" />
</template>
