<script setup lang="ts">
import type { CodexQuotaOverdraftWindow } from '@/api/modules/accounts'
import { computed } from 'vue'
import { codexQuotaOverdraftStatsText } from './presenter'

const props = defineProps<{
  window: CodexQuotaOverdraftWindow
}>()

const stats = computed(() => codexQuotaOverdraftStatsText(props.window))
const title = computed(() => [
  props.window.startedAt ? `开始：${new Date(props.window.startedAt).toLocaleString('zh-CN')}` : '',
  props.window.recoverAt ? `预计恢复：${new Date(props.window.recoverAt).toLocaleString('zh-CN')}` : '',
].filter(Boolean).join(' · '))
</script>

<template>
  <div
    v-if="window.active"
    class="flex min-w-0 items-center gap-1.5 text-[9px] leading-3 text-cp-error-text"
    :title="title"
  >
    <span class="shrink-0 rounded-sm bg-cp-error-container px-1.5 py-0.5 font-heavy">透支中</span>
    <span v-if="stats" class="min-w-0 truncate font-mono font-emphasis text-cp-text-tertiary">
      {{ stats }}
    </span>
  </div>
</template>
