<script setup lang="ts">
import { computed } from 'vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'

import BaseSelect from '@/components/base/BaseSelect.vue'
import { useProxyCatalog } from '@/composables/useProxyCatalog'
import { t } from '@/i18n'

const props = withDefaults(defineProps<{ accountId?: string, currentProxyId?: string | null, endpoint?: string | null, disabled?: boolean, preserve?: boolean, error?: string }>(), { preserve: true })

const mode = defineModel<string>('mode', { required: true })
const proxyId = defineModel<string>('proxyId', { required: true })
const { proxies, loading } = useProxyCatalog()
// 按绑定 ID 识别代理，避免相同脱敏地址下的不同认证配置被误认
const currentSelection = computed(() => props.currentProxyId || (props.endpoint ? 'preserve' : 'direct'))
const options = computed(() => [
  ...(props.preserve && !props.accountId
    ? [
        {
          label: t('accounts.proxy.keepCurrent'),
          value: 'preserve',
          description: t('accounts.proxy.keepExisting'),
        },
      ]
    : []),
  ...(props.preserve && props.accountId && props.endpoint && !proxies.value.some(proxy => proxy.id === props.currentProxyId)
    ? [
        { label: props.endpoint, value: currentSelection.value },
      ]
    : []),
  {
    label: t('accounts.proxy.directConnection'),
    value: 'direct',
    description: t('accounts.proxy.notUsingProxy'),
  },
  ...proxies.value.map(proxy => ({
    label: `${proxy.name.trim() || proxy.endpoint}${!proxy.enabled
      ? t('accounts.proxy.disabledSuffix')
      : proxy.lastTest?.success
        ? ''
        : proxy.lastTest
          ? t('accounts.proxy.testFailed')
          : t('accounts.proxy.notTested')}`,
    value: proxy.id,
    description: proxy.name.trim()
      ? proxy.endpoint
      : undefined,
    disabled: !proxy.enabled,
  })),
])
const selection = computed({
  get: () => mode.value === 'proxy'
    ? proxyId.value
    : mode.value === 'preserve' && props.accountId
      ? currentSelection.value
      : mode.value,
  set: (value: string) => {
    // 显示已绑定代理，未改选择时仍省略写入，保留禁用代理及仅地址配置
    const nextMode = props.preserve && props.accountId && value === currentSelection.value
      ? 'preserve'
      : value === 'preserve' || value === 'direct'
        ? value
        : 'proxy'
    mode.value = nextMode
    proxyId.value = nextMode === 'proxy'
      ? value
      : ''
  },
})
</script>

<template>
  <BaseFormItem :label="t('accounts.proxy.outboundTunnel')" :error="error">
    <template v-if="$slots.extra" #extra>
      <slot name="extra" />
    </template>
    <BaseSelect :key="disabled ? 'disabled' : 'enabled'" v-model="selection" class="w-full" :options="options" :disabled="disabled || loading" :aria-label="t('accounts.proxy.outboundTunnel')" />
  </BaseFormItem>
</template>
