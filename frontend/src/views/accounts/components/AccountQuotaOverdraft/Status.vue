<script setup lang="ts">
import type { CodexQuotaOverdraftState } from '@/api/modules/accounts'
import { computed } from 'vue'
import { useUiClock } from '@/composables/useUiClock'
import { codexQuotaOverdraftStatusView } from './presenter'

const props = defineProps<{
  state: CodexQuotaOverdraftState
}>()

const now = useUiClock()
const view = computed(() => codexQuotaOverdraftStatusView(props.state, now.value.getTime()))
const toneClasses = computed(() => ({
  danger: 'bg-cp-error-container text-cp-error-text',
  success: 'bg-cp-success-container text-cp-success-text',
  warning: 'bg-cp-warning-container text-cp-warning-text',
})[view.value.tone])
</script>

<template>
  <div
    class="flex min-w-0 items-center gap-1.5 text-[9px] leading-3"
    :class="view.tone === 'danger' ? 'text-cp-error-text' : view.tone === 'warning' ? 'text-cp-warning-text' : 'text-cp-success-text'"
    :title="view.title"
    :aria-label="[view.label, view.detail].filter(Boolean).join('，')"
  >
    <span class="shrink-0 rounded-sm px-1.5 py-0.5 font-heavy" :class="toneClasses">
      {{ view.label }}
    </span>
    <span v-if="view.detail" class="min-w-0 truncate font-mono font-emphasis text-cp-text-tertiary">
      {{ view.detail }}
    </span>
  </div>
</template>
