<script setup lang="ts">
import type { Excel403Action } from '@/utils/excel-settings'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { EXCEL_403_OPTIONS } from '@/utils/excel-settings'

defineProps<{ disabled?: boolean }>()
const model = defineModel<Excel403Action>({ required: true })

function update(value: string) {
  if (value === 'none' || value === 'pause_account' || value === 'disable_excel')
    model.value = value
}
</script>

<template>
  <BaseSelect
    class="w-full"
    :model-value="model"
    :options="EXCEL_403_OPTIONS"
    :disabled="disabled"
    aria-label="Excel遇到HTTP 403"
    @update:model-value="update"
  />
</template>
