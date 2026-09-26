<script setup lang="ts">
import { computed } from 'vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { DEFAULT_EXCEL_MODELS_INPUT } from '@/utils/excel-defaults'

defineProps<{ disabled?: boolean }>()
const followGlobal = defineModel<boolean>('followGlobal', { default: true })
const models = defineModel<string>('models', { default: DEFAULT_EXCEL_MODELS_INPUT })
const mode = computed({
  get: () => followGlobal.value ? 'global' : 'custom',
  set: (value: string) => { followGlobal.value = value === 'global' },
})
</script>

<template>
  <div class="grid min-w-0 gap-2">
    <BaseSelect
      v-model="mode"
      aria-label="Excel 模型来源"
      :options="[{ label: '跟随全局', value: 'global' }, { label: '自定义', value: 'custom' }]"
      :disabled="disabled"
    />
    <BaseInput
      v-if="!followGlobal"
      v-model="models"
      aria-label="Excel 模型"
      :placeholder="DEFAULT_EXCEL_MODELS_INPUT"
      :disabled="disabled"
    />
  </div>
</template>
