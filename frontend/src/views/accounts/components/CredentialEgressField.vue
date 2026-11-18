<script setup lang="ts">
import { computed } from 'vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { useProxyCatalog } from '@/composables/useProxyCatalog'

const props = defineProps<{ disabled: boolean, template?: boolean }>()
const proxyId = defineModel<string | null>()
const { proxies, loading } = useProxyCatalog()
const options = computed(() => [
  ...(props.template ? [{ value: 'keep', label: '保留账号原设置' }] : []),
  { value: 'follow', label: '跟随账号出口' },
  ...proxies.value.filter(p => p.enabled || p.id === proxyId.value).map(p => ({ value: `proxy:${p.id}`, label: p.enabled ? p.name : `${p.name}（已停用）`, disabled: !p.enabled })),
  ...(proxyId.value && !proxies.value.some(p => p.id === proxyId.value) ? [{ value: `proxy:${proxyId.value}`, label: loading.value ? '读取代理中' : '原代理不可用', disabled: true }] : []),
])
const selected = computed({
  get: () => proxyId.value ? `proxy:${proxyId.value}` : props.template && proxyId.value === undefined ? 'keep' : 'follow',
  set: (key: string) => { proxyId.value = key === 'keep' ? undefined : key === 'follow' ? null : key.slice(6) },
})
</script>

<template>
  <BaseFormItem label="凭据维护出口">
    <BaseSelect v-model="selected" :options="options" :disabled="disabled || loading" aria-label="凭据维护出口" />
  </BaseFormItem>
</template>
