<script setup lang="ts" generic="Value extends string">
import type { BuiltinProvider } from '@/providers/catalog'
import { computed } from 'vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'
import { t } from '@/i18n'
import { formatProviderLabel, PROVIDER_IDS, providerIcon } from '@/providers/catalog'

const props = withDefaults(defineProps<{ providers?: readonly Value[], disabled?: boolean, ariaLabel?: string }>(), { disabled: false })
const provider = defineModel<Value>({ required: true })
const options = computed(() => (props.providers ?? PROVIDER_IDS).map(value => ({ value, label: formatProviderLabel(value), icon: PROVIDER_IDS.includes(value as BuiltinProvider) ? providerIcon(value as BuiltinProvider) : undefined })))
function select(value: string) {
  if (options.value.some(option => option.value === value))
    provider.value = value as Value
}
</script>

<template>
  <BaseSegmented :model-value="provider" :options="options" :disabled="disabled" :label="ariaLabel ?? t('common.providers.select')" @update:model-value="select" />
</template>
