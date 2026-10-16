<script setup lang="ts">
import type { QualityMonitoring } from '@/api/modules/quality-ops'
import { Activity } from '@lucide/vue'
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import { formatDateTime } from '@/utils/date'
import { monitoringPresentation } from './monitoring'
import { qualityStatusColor, qualityVerdictLabel } from './presentation'

const props = defineProps<{ accountId: string, monitor: QualityMonitoring }>()
const status = computed(() => monitoringPresentation(props.monitor))
const verdict = computed(() => qualityVerdictLabel(props.monitor.lastStatus, props.monitor.detectionMode))
const title = computed(() => [
  status.value.label,
  `${verdict.value} · ${formatDateTime(props.monitor.lastRunAt)}`,
  props.monitor.sourceTemplate ? `来源模板：${props.monitor.sourceTemplate.name}` : '独立检测规则',
  props.monitor.enabled ? `下次检测：${formatDateTime(props.monitor.nextRunAt)}` : '定时检测已暂停',
].filter(Boolean).join(' · '))
</script>

<template>
  <RouterLink :to="{ path: '/quality-ops', query: { accountId, ruleId: monitor.ruleId } }" :title="title" class="mt-1 inline-flex max-w-full min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5 whitespace-normal text-cp-xs hover:underline" data-swipe-select-ignore @click.stop>
    <span :class="status.tone" class="inline-flex shrink-0 items-center gap-1"><Activity class="size-3 shrink-0" />{{ status.label }}</span>
    <span :class="qualityStatusColor(monitor.lastStatus)" data-quality-verdict>{{ verdict }}</span>
  </RouterLink>
</template>
