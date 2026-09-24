<script setup lang="ts">
import type { AccountGroup, GroupMonitorItem } from '@/api'
import { Pin, PinOff, Settings } from '@lucide/vue'
import { computed } from 'vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import { monitorEta, monitorExpiryHint, monitorMoney } from './group-monitor-presentation'

const props = defineProps<{
  group: AccountGroup
  snapshot?: GroupMonitorItem
  pinned: boolean
  pinDisabled: boolean
  stale: boolean
  loading: boolean
  now: number
}>()
defineEmits<{ pin: [], settings: [] }>()

const expired = computed(() => !!props.snapshot?.earliestResetAt && Date.parse(props.snapshot.earliestResetAt) <= props.now)
const status = computed(() => !props.group.enabled ? 'disabled' : expired.value ? 'unknown' : props.snapshot?.remainingStatus ?? 'unknown')
const metrics = computed(() => [
  { label: '预计剩余额度', value: monitorMoney(expired.value ? null : props.snapshot?.remainingUsd, status.value) },
  { label: '预计过期额度', value: monitorMoney(expired.value ? null : props.snapshot?.expectedExpiryUsd, !props.group.enabled ? 'disabled' : expired.value ? 'unknown' : props.snapshot?.expiryStatus ?? status.value), hint: monitorExpiryHint(props.snapshot?.expiryStatus) },
  { label: '每分钟消耗', value: monitorMoney(props.snapshot?.consumeUsdPerMinute, 'unknown', 4), unit: '/分' },
  { label: '预计可支撑', value: monitorEta(expired.value ? null : props.snapshot?.etaMinutes, !props.group.enabled ? 'disabled' : expired.value ? 'unknown' : props.snapshot?.etaStatus ?? status.value) },
])
const percentage = computed(() => props.snapshot?.usedSlots != null && props.snapshot.totalSlots > 0
  ? Math.min(100, props.snapshot.usedSlots / props.snapshot.totalSlots * 100)
  : 0)
const alerting = computed(() => Boolean(props.snapshot?.activeAlerts?.length))
</script>

<template>
  <BaseCard as="article" padding="none" class="monitor-group" :aria-label="group.name">
    <div class="flex min-w-0 items-center gap-1.5">
      <span class="size-2 shrink-0 rounded-full" :style="{ backgroundColor: group.color }" />
      <h3 class="min-w-0 flex-1 truncate text-cp-xs font-heavy text-cp-text" :title="group.name">
        {{ group.name }}
      </h3>
      <BaseIconButton :label="`设置 ${group.name} 预警`" size="sm" class="monitor-action" @click="$emit('settings')">
        <Settings class="size-3.5" :class="alerting ? 'text-cp-error-text' : stale || expired ? 'text-cp-warning-text' : ''" />
      </BaseIconButton>
      <BaseIconButton
        :label="`${pinned ? '取消置顶' : '置顶'} ${group.name}`"
        size="sm"
        class="monitor-action"
        :pressed="pinned"
        :disabled="pinDisabled"
        @click="$emit('pin')"
      >
        <component :is="pinned ? PinOff : Pin" class="size-3.5" />
      </BaseIconButton>
      <slot name="actions" />
    </div>
    <dl class="monitor-metrics">
      <div v-for="metric in metrics" :key="metric.label" class="min-w-0">
        <dt class="monitor-label text-cp-text-tertiary">
          {{ metric.label }}
        </dt>
        <dd class="monitor-value" :title="metric.hint ?? metric.value">
          {{ !snapshot ? (loading ? '读取中' : '采样中') : metric.value }}
          <span v-if="metric.unit && snapshot?.consumeUsdPerMinute != null" class="text-cp-xs font-normal text-cp-text-tertiary">{{ metric.unit }}</span>
        </dd>
      </div>
    </dl>
    <div class="monitor-concurrency mt-auto flex items-center gap-2 text-cp-xs" :class="stale ? 'text-cp-text-tertiary' : 'text-cp-text-secondary'">
      <span>并发</span>
      <strong class="shrink-0 font-mono font-emphasis">{{ snapshot?.usedSlots ?? '—' }} / {{ snapshot?.usedSlots == null ? '—' : snapshot.totalSlots }}</strong>
      <div
        class="h-1 min-w-3 flex-1 overflow-hidden rounded-sm bg-cp-fill-quaternary"
        role="progressbar"
        :aria-label="`${group.name}共享并发使用率`"
        :aria-valuenow="snapshot?.usedSlots == null ? undefined : percentage"
        :aria-valuetext="snapshot?.usedSlots == null ? '未知' : `${snapshot.usedSlots} / ${snapshot.totalSlots}`"
        :aria-valuemin="0"
        :aria-valuemax="100"
      >
        <div class="h-full" :class="percentage >= 90 ? 'bg-cp-warning' : 'bg-cp-success'" :style="{ width: `${percentage}%` }" />
      </div>
    </div>
  </BaseCard>
</template>

<style scoped>
.monitor-group {
  display: flex;
  min-width: 0;
  min-height: 128px;
  flex-direction: column;
  gap: 3px;
  padding: 7px 10px;
}
.monitor-action {
  width: 24px;
  height: 24px;
}
.monitor-label {
  font-size: 11px;
  line-height: 14px;
}
.monitor-concurrency {
  line-height: 14px;
}
.monitor-metrics {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 3px 10px;
  margin: 0;
}
.monitor-value {
  margin: 0;
  overflow-wrap: anywhere;
  font-size: 15px;
  line-height: 18px;
  font-weight: 650;
  font-variant-numeric: tabular-nums;
  letter-spacing: 0;
}
</style>
