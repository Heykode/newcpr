<script setup lang="ts">
import type { UsageDisplayRecord } from '../utils/records'
import { Bot, SquareDashedBottomCode } from '@lucide/vue'

import { t } from '@/i18n'
import { usageIsReview, usageIsSubagent, usageReasoningEffort } from '../utils/records'

defineProps<{
  record: UsageDisplayRecord
}>()
</script>

<template>
  <span
    class="inline-flex items-center gap-1 whitespace-nowrap text-cp-sm font-bold text-cp-text"
  >
    <span>{{ usageReasoningEffort(record) }}</span>
    <span
      v-if="usageIsSubagent(record)"
      class="inline-flex shrink-0 text-cp-purple-text"
      :title="t('usage.reasoning.subagentRequest')"
      :aria-label="t('usage.reasoning.subagentRequest')"
    >
      <Bot class="size-3.5" stroke-width="2.2" />
    </span>
    <span
      v-if="usageIsReview(record)"
      class="inline-flex shrink-0 text-cp-purple-text"
      :title="t('usage.reasoning.reviewSubagentRequest')"
      :aria-label="t('usage.reasoning.reviewSubagentRequest')"
    >
      <SquareDashedBottomCode class="size-3.25" stroke-width="2.2" />
    </span>
  </span>
</template>
