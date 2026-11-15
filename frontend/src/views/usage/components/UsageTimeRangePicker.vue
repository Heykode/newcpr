<script setup lang="ts">
import type { UsageTimeRange, UsageTimeRangeParams } from '../composables/useUsageTimeRange'
import type { UsageFilterDraft } from '../utils/filters'
import { CalendarDays, ChevronDown } from '@lucide/vue'
import dayjs from 'dayjs'
import { computed, nextTick, ref, watch } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BasePopover from '@/components/base/BasePopover.vue'

const props = defineProps<{ range: UsageTimeRangeParams }>()
const model = defineModel<UsageFilterDraft>({ required: true })
const open = ref(false)
const panel = ref<HTMLFormElement>()
const trigger = ref<InstanceType<typeof BaseButton>>()
const start = ref('')
const end = ref('')
const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone
const shortcuts: { label: string, value: UsageTimeRange }[] = [
  { label: '今天', value: 'today' },
  { label: '最近 7 天', value: '7d' },
  { label: '最近 30 天', value: '30d' },
]
const custom = computed(() => Boolean(model.value.startTime || model.value.endTime))
const effective = computed(() => ({
  startTime: model.value.startTime || props.range.startTime,
  endTime: model.value.endTime || props.range.endTime,
}))
const label = computed(() => custom.value ? '自定义' : shortcuts.find(item => item.value === (model.value.timeRange || 'today'))?.label || '今天')
const rangeLabel = computed(() => {
  const first = dayjs(effective.value.startTime)
  const last = dayjs(effective.value.endTime)
  return first.isSame(last, 'day')
    ? `${first.format('MM/DD')} ${first.format('HH:mm')}–${last.format('HH:mm')}`
    : `${first.format('MM/DD HH:mm')}–${last.format('MM/DD HH:mm')}`
})
const invalid = computed(() => !start.value || !end.value
  || !dayjs(start.value).isValid() || !dayjs(end.value).isValid()
  || !dayjs(end.value).isAfter(dayjs(start.value)))
watch(open, async (visible) => {
  if (visible) {
    start.value = dayjs(effective.value.startTime).format('YYYY-MM-DDTHH:mm:ss')
    end.value = dayjs(effective.value.endTime).format('YYYY-MM-DDTHH:mm:ss')
    await nextTick()
    if (open.value)
      panel.value?.querySelector('input')?.focus()
  }
  else if (panel.value?.contains(document.activeElement)) {
    trigger.value?.$el.focus()
  }
})
function shortcut(value: UsageTimeRange) {
  model.value = { ...model.value, timeRange: value, startTime: '', endTime: '' }
  open.value = false
}
function yesterday() {
  const today = dayjs().startOf('day')
  model.value = { ...model.value, startTime: today.subtract(1, 'day').toISOString(), endTime: today.toISOString() }
  open.value = false
}
function apply() {
  if (invalid.value)
    return
  // Commit both boundaries together; incomplete edits never reach the queries.
  model.value = { ...model.value, startTime: dayjs(start.value).toISOString(), endTime: dayjs(end.value).toISOString() }
  open.value = false
}
</script>

<template>
  <div class="min-w-0">
    <span class="mb-1 block text-cp-xs text-cp-text-secondary">时间范围</span>
    <BasePopover v-model="open" placement="bottom-start" role="dialog" class="w-full">
      <template #trigger>
        <BaseButton ref="trigger" class="time-range-trigger w-full" aria-label="筛选时间范围" :aria-expanded="open" aria-haspopup="dialog" :title="`${effective.startTime} — ${effective.endTime} · ${timezone}`">
          <CalendarDays :size="15" class="shrink-0" />
          <span class="flex min-w-0 flex-1 flex-wrap items-center gap-x-2 gap-y-1 text-left text-cp-xs"><span class="shrink-0 font-medium">{{ label }}</span><span class="font-normal tabular-nums text-cp-text-secondary">{{ rangeLabel }}</span></span>
          <ChevronDown :size="13" class="shrink-0" />
        </BaseButton>
      </template>
      <form ref="panel" class="w-120 max-w-[calc(100vw-32px)] space-y-4 p-4" aria-label="时间范围" @submit.prevent="apply">
        <div class="flex flex-wrap gap-2">
          <BaseButton v-for="item in shortcuts" :key="item.value" size="sm" :variant="!custom && (model.timeRange || 'today') === item.value ? 'soft' : 'secondary'" @click="shortcut(item.value)">
            {{ item.label }}
          </BaseButton>
          <BaseButton size="sm" @click="yesterday">
            昨天
          </BaseButton>
        </div>
        <div class="grid grid-cols-[repeat(auto-fit,minmax(min(100%,180px),1fr))] gap-3">
          <BaseFormItem label="开始时间">
            <BaseInput v-model="start" type="datetime-local" step="1" aria-label="开始时间" />
          </BaseFormItem>
          <BaseFormItem label="结束时间">
            <BaseInput v-model="end" type="datetime-local" step="1" aria-label="结束时间" />
          </BaseFormItem>
        </div>
        <p class="m-0 text-cp-xs text-cp-text-secondary">
          时区：{{ timezone }} · 同步更新图表和请求列表
        </p>
        <p v-if="invalid" role="status" class="m-0 text-cp-xs text-cp-error-text">
          请选择完整时间，结束时间须晚于开始时间。
        </p>
        <div class="flex justify-end gap-2">
          <BaseButton @click="open = false">
            取消
          </BaseButton>
          <BaseButton type="submit" variant="primary" :disabled="invalid">
            应用
          </BaseButton>
        </div>
      </form>
    </BasePopover>
  </div>
</template>

<style scoped>
.time-range-trigger {
  height: auto;
  min-height: var(--cp-control-height);
  padding: 8px 10px;
}
.time-range-trigger :deep(> span:last-child) {
  width: 100%;
}
</style>
