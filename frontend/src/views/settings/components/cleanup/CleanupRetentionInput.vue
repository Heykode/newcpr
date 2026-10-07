<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseNumberInput from '@/components/base/BaseNumberInput.vue'

defineProps<{ label: string, max: number, disabled: boolean }>()
const model = defineModel<number>({ required: true })
const lastDays = ref(1)
watch(model, (days) => {
  if (days > 0)
    lastDays.value = days
}, { immediate: true, flush: 'sync' })
const all = computed({
  get: () => model.value === 0,
  set: (value: boolean) => { model.value = value ? 0 : lastDays.value },
})
const days = computed({
  get: () => model.value === 0 ? lastDays.value : model.value,
  set: (value: number) => { model.value = Math.max(1, value) },
})
</script>

<template>
  <div role="group" :aria-label="`${label}保留设置`" class="grid min-w-0 justify-items-start gap-3">
    <BaseNumberInput v-model="days" :label="`${label}保留天数`" unit="天" :min="1" :max="max" :disabled="disabled || all" class="max-w-full" />
    <BaseCheckbox v-model="all" label="全部清理" show-label :disabled="disabled" />
  </div>
</template>
