<script setup lang="ts">
import { CalendarDays, ChevronDown, ChevronLeft, ChevronRight } from '@lucide/vue'
import { useEventListener, useIntervalFn, useNow } from '@vueuse/core'
import dayjs from 'dayjs'
import { computed, nextTick, ref, useId, watch } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BasePopover from '@/components/base/BasePopover.vue'

defineProps<{ disabled: boolean }>()
const value = defineModel<string>({ required: true })
const open = ref(false)
const trigger = ref<HTMLButtonElement>()
const panel = ref<HTMLElement>()
const panelId = useId()
const now = useNow({ scheduler: callback => useIntervalFn(callback, 10000) })
const today = computed(() => dayjs(now.value).format('YYYY-MM-DD'))
const zone = Intl.DateTimeFormat().resolvedOptions().timeZone
const format = 'YYYY-MM-DDTHH:mm'
const month = ref(dayjs().startOf('month'))
const selectedDay = ref('')
const hour = ref('')
const minute = ref('')
const days = computed(() => {
  const offset = (month.value.day() + 6) % 7
  return Array.from({ length: 42 }, (_, i) => {
    const n = i - offset + 1
    return n >= 1 && n <= month.value.daysInMonth() ? month.value.date(n).format('YYYY-MM-DD') : ''
  })
})
const candidate = computed(() => {
  if (!selectedDay.value || !/^\d{1,2}$/.test(hour.value) || !/^\d{1,2}$/.test(minute.value))
    return ''
  return `${selectedDay.value}T${hour.value.padStart(2, '0')}:${minute.value.padStart(2, '0')}`
})
const valid = computed(() => {
  const date = dayjs(candidate.value)
  return date.isValid() && date.format(format) === candidate.value && date.valueOf() > now.value.getTime()
})
watch(open, async (visible) => {
  if (!visible)
    return
  selectedDay.value = value.value.slice(0, 10)
  hour.value = value.value.slice(11, 13)
  minute.value = value.value.slice(14, 16)
  month.value = dayjs(value.value || undefined).startOf('month')
  await nextTick()
  panel.value?.focus({ preventScroll: true })
})
function close() {
  open.value = false
  trigger.value?.focus({ preventScroll: true })
}
function apply() {
  if (valid.value) {
    value.value = candidate.value
    close()
  }
}
function tab(event: KeyboardEvent) {
  const controls = panel.value?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled)')
  if (!controls?.length)
    return
  const first = controls[0]
  const last = controls[controls.length - 1]
  if (event.shiftKey && [panel.value, first].includes(document.activeElement as HTMLElement)) {
    event.preventDefault()
    last?.focus()
  }
  else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault()
    first?.focus()
  }
}
useEventListener(panel, 'keydown', (event) => {
  if (event.key === 'Escape') {
    event.stopPropagation()
    event.preventDefault()
    close()
  }
  else if (event.key === 'Tab') {
    tab(event)
  }
})
</script>

<template>
  <div class="grid min-w-0 gap-2 text-cp-sm">
    <div class="flex flex-wrap items-baseline justify-between gap-x-2">
      <span class="font-medium">执行时间</span>
      <span class="text-cp-xs text-cp-text-tertiary">{{ zone }}</span>
    </div>
    <BasePopover v-model="open" class="w-full" placement="bottom-start" :disabled="disabled">
      <template #trigger>
        <button ref="trigger" type="button" class="flex min-h-10 w-full items-center gap-2 rounded-cp border border-cp-border bg-cp-bg-container px-3 text-left hover:bg-cp-fill-quaternary focus-visible:outline-cp-primary" aria-haspopup="dialog" :aria-controls="open ? panelId : undefined" :aria-expanded="open" :disabled="disabled">
          <CalendarDays class="size-4 shrink-0 text-cp-text-secondary" />
          <span class="flex-1 tabular-nums">{{ value ? value.replace('T', ' ') : '选择日期和时间' }}</span>
          <ChevronDown class="size-4 text-cp-text-secondary" :class="{ 'rotate-180': open }" />
        </button>
      </template>
      <div :id="panelId" ref="panel" role="dialog" aria-label="选择执行时间" tabindex="-1" class="w-72 max-w-[calc(100vw-24px)] max-h-[calc(100dvh-24px)] overflow-auto overscroll-contain p-3 text-cp-sm outline-none">
        <div class="flex items-center justify-between">
          <BaseIconButton label="上个月" :disabled="!month.isAfter(dayjs(today), 'month')" @click="month = month.subtract(1, 'month')">
            <ChevronLeft />
          </BaseIconButton>
          <strong class="tabular-nums">{{ month.format('YYYY 年 M 月') }}</strong>
          <BaseIconButton label="下个月" @click="month = month.add(1, 'month')">
            <ChevronRight />
          </BaseIconButton>
        </div>
        <div class="grid grid-cols-7 text-center">
          <span v-for="w in '一二三四五六日'" :key="w" class="py-1 text-cp-xs text-cp-text-tertiary">{{ w }}</span>
          <template v-for="(day, i) in days" :key="day || i">
            <span v-if="!day" class="h-8" />
            <button v-else type="button" class="h-8 rounded-cp-sm tabular-nums focus-visible:outline-cp-primary disabled:opacity-35" :class="day === selectedDay ? 'bg-(--cp-button-primary-bg) text-(--cp-button-primary-color)' : 'hover:bg-cp-fill-tertiary'" :disabled="day < today" :aria-label="day" :aria-pressed="day === selectedDay" :aria-current="day === today ? 'date' : undefined" @click="selectedDay = day">
              {{ Number(day.slice(-2)) }}
            </button>
          </template>
        </div>
        <div class="mt-2 flex items-center gap-1.5 border-t border-cp-border-secondary pt-2">
          <BaseInput v-model="hour" class="w-14" inputmode="numeric" maxlength="2" aria-label="小时" placeholder="时" @keydown.enter.prevent="apply" />
          <span aria-hidden="true">:</span>
          <BaseInput v-model="minute" class="w-14" inputmode="numeric" maxlength="2" aria-label="分钟" placeholder="分" @keydown.enter.prevent="apply" />
          <BaseButton class="ml-auto" variant="primary" size="sm" :disabled="!valid" @click="apply">
            确定
          </BaseButton>
        </div>
        <p v-if="candidate && !valid" role="status" class="mb-0 mt-2 text-cp-xs text-cp-error">
          请选择有效且晚于现在的时间。
        </p>
      </div>
    </BasePopover>
  </div>
</template>
