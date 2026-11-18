<script setup lang="ts">
import type { EnrollmentEntry } from '../enrollment-plan'
import { formatDateTime } from '@/utils/date'
import { enrollmentLabels } from '../enrollment-plan'

defineProps<{ row: EnrollmentEntry }>()
</script>

<template>
  <div class="grid min-w-0 gap-1 text-cp-sm" aria-live="polite">
    <strong class="font-medium" :class="row.enrollmentPlan?.state === 'failed' ? 'text-cp-error' : row.enrollmentPlan?.state === 'verification' ? 'text-cp-warning' : 'text-cp-text'">
      {{ row.enrollmentPlan ? enrollmentLabels[row.enrollmentPlan.state] : '待安排' }}
    </strong>
    <time v-if="row.enrollmentPlan?.state === 'scheduled' && row.enrollmentPlan.when" class="text-cp-xs tabular-nums text-cp-text-secondary" :datetime="row.enrollmentPlan.when">
      {{ formatDateTime(row.enrollmentPlan.when).slice(0, 16) }}
    </time>
  </div>
</template>
