<script setup lang="ts">
import type { QualityMonitoring } from '@/api/modules/quality-ops'
import { Activity } from '@lucide/vue'
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import { formatDateTime } from '@/utils/date'
import { monitoringPresentation } from './monitoring'

const props = defineProps<{ accountId: string, monitor: QualityMonitoring }>()
const status = computed(() => monitoringPresentation(props.monitor))
const title = computed(() => [
  status.value.label,
  props.monitor.sourceTemplate ? `来源模板：${props.monitor.sourceTemplate.name}` : '独立检测规则',
  props.monitor.enabled ? `下次检测：${formatDateTime(props.monitor.nextRunAt)}` : '定时检测已暂停',
].join(' · '))
</script>

<template>
  <RouterLink :to="{ path: '/quality-ops', query: { accountId, ruleId: monitor.ruleId } }" :title="title" :class="status.tone" class="mt-1 inline-flex max-w-full items-center gap-1 text-cp-xs hover:underline" data-swipe-select-ignore @click.stop>
    <Activity class="size-3 shrink-0" />{{ status.label }}
  </RouterLink>
</template>
