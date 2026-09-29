<script setup lang="ts">
import { computed } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import FormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import { DEFAULT_QUALITY_INTERVAL_SECONDS, MAX_QUALITY_INTERVAL_SECONDS, MIN_QUALITY_INTERVAL_SECONDS } from './schedule'

defineProps<{ cron?: string, timezone?: string }>()
const interval = defineModel<number | null | undefined>()
const seconds = computed({
  get: () => interval.value === 0 ? '' : String(interval.value ?? DEFAULT_QUALITY_INTERVAL_SECONDS),
  set: (value: string) => { interval.value = Number.isFinite(Number(value)) ? Number(value) : 0 },
})
</script>

<template>
  <div class="grid min-w-0 gap-3">
    <template v-if="interval == null">
      <p class="break-words text-cp-sm text-cp-text-secondary">
        保留原定时：{{ cron }}（{{ timezone }}）。未切换时，保存不会改变原检测频率。
      </p>
      <BaseButton @click="interval = DEFAULT_QUALITY_INTERVAL_SECONDS">
        改为秒数间隔
      </BaseButton>
    </template>
    <template v-else>
      <FormItem label="检测频率" required>
        <BaseInput v-model="seconds" type="number" aria-label="检测频率" inputmode="numeric" :min="MIN_QUALITY_INTERVAL_SECONDS" :max="MAX_QUALITY_INTERVAL_SECONDS" :step="1">
          <template #suffix>
            秒
          </template>
        </BaseInput>
      </FormItem>
      <p class="text-xs text-cp-text-secondary">
        默认 60 秒，最少 5 秒。每轮结束后等待所填秒数；后台约每 5 秒检查到期任务，繁忙时顺延，不重叠执行。
      </p>
    </template>
  </div>
</template>
