<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import FormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { readSchedule, scheduleOptions, scheduleSummary, writeSchedule } from './schedule'

const cron = defineModel<string>({ required: true })
const initial = readSchedule(cron.value)
const frequency = ref(initial.frequency)
const time = ref(initial.time)
const summary = computed(() => scheduleSummary(frequency.value, time.value))

function changeFrequency(value: string) {
  frequency.value = value
  const next = writeSchedule(value, time.value)
  if (next)
    cron.value = next
}
function changeTime(value: string) {
  time.value = value
  const next = writeSchedule(frequency.value, value)
  if (next)
    cron.value = next
}
watch(cron, (value) => {
  // Keep the advanced editor open while typing an expression matching a preset.
  if (frequency.value !== 'custom') {
    const parsed = readSchedule(value)
    frequency.value = parsed.frequency
    time.value = parsed.time
  }
})
</script>

<template>
  <div class="grid min-w-0 gap-3">
    <FormItem label="检测频率" required>
      <BaseSelect :model-value="frequency" :options="scheduleOptions" @update:model-value="changeFrequency" />
    </FormItem>
    <FormItem v-if="frequency === 'daily'" label="每天检测时间" required>
      <BaseInput type="time" :model-value="time" @update:model-value="changeTime" />
    </FormItem>
    <FormItem v-if="frequency === 'custom'" label="Cron 表达式" required>
      <BaseInput v-model="cron" placeholder="0 */6 * * *" />
    </FormItem>
    <p v-if="summary" class="text-xs text-cp-text-secondary">
      {{ summary }}（所选时区）
    </p>
  </div>
</template>
