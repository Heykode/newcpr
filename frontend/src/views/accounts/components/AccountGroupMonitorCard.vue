<script setup lang="ts">
import type { AccountGroup, GroupMonitorItem } from '@/api'
import { Info, Pin, PinOff } from '@lucide/vue'
import { computed } from 'vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BasePopover from '@/components/base/BasePopover.vue'
import { monitorEta, monitorMoney } from './group-monitor-presentation'

const props = defineProps<{
  group: AccountGroup
  snapshot?: GroupMonitorItem
  pinned: boolean
  pinDisabled: boolean
  stale: boolean
  loading: boolean
  now: number
}>()
defineEmits<{ pin: [] }>()

const expired = computed(() => !!props.snapshot?.earliestResetAt && Date.parse(props.snapshot.earliestResetAt) <= props.now)
const status = computed(() => !props.group.enabled ? 'disabled' : expired.value ? 'unknown' : props.snapshot?.remainingStatus ?? 'unknown')
const metrics = computed(() => [
  { label: '预计剩余额度', value: monitorMoney(expired.value ? null : props.snapshot?.remainingUsd, status.value) },
  { label: '预计过期额度', value: monitorMoney(expired.value ? null : props.snapshot?.expectedExpiryUsd, expired.value ? 'unknown' : props.snapshot?.expiryStatus ?? status.value) },
  { label: '每分钟消耗', value: monitorMoney(props.snapshot?.consumeUsdPerMinute, 'unknown', 4), unit: '/分' },
  { label: '预计可支撑', value: monitorEta(expired.value ? null : props.snapshot?.etaMinutes, expired.value ? 'unknown' : props.snapshot?.etaStatus ?? status.value) },
])
const percentage = computed(() => props.snapshot?.usedSlots != null && props.snapshot.totalSlots > 0
  ? Math.min(100, props.snapshot.usedSlots / props.snapshot.totalSlots * 100)
  : 0)
</script>

<template>
  <BaseCard as="article" padding="none" class="monitor-group" :aria-label="group.name">
    <div class="flex min-w-0 items-center gap-1.5">
      <span class="size-2 shrink-0 rounded-full" :style="{ backgroundColor: group.color }" />
      <h3 class="min-w-0 flex-1 truncate text-cp-xs font-heavy text-cp-text" :title="group.name">
        {{ group.name }}
      </h3>
      <BasePopover trigger="hover-click" placement="bottom-end" :hover-delay="150">
        <template #trigger>
          <BaseIconButton label="查看监控口径" size="sm" class="monitor-action">
            <Info class="size-3.5" :class="stale || expired ? 'text-cp-warning-text' : ''" />
          </BaseIconButton>
        </template>
        <div class="max-w-[min(320px,calc(100vw-32px))] space-y-2 p-3 text-cp-sm">
          <p class="break-words font-heavy">
            {{ group.name }}
          </p>
          <p v-if="stale" class="text-cp-warning-text">
            数据未更新，当前保留上次观测。
          </p>
          <p v-if="expired" class="text-cp-warning-text">
            额度窗口已到期，等待新的有效观测。
          </p>
          <p>7D 额度估算覆盖 {{ snapshot?.estimatedAccounts ?? 0 }} / {{ snapshot?.eligibleAccounts ?? 0 }} 个可调度账号；优先按自身本轮消费与已用比例计算。新号无自身估值时，参考同 Provider、套餐及窗口最新最多 3 个有效账号的平均总额度，再按自身已用比例计算剩余。不包含未来重置补充，短期限额仍可能限制使用。</p>
          <p v-if="snapshot?.remainingStatus === 'partial'">
            部分可调度账号暂无有效额度估值，当前仅汇总已可计算账号。
          </p>
          <p v-if="snapshot?.lowSample">
            样本较少，估算可能波动。
          </p>
          <p>预计过期额度依据同 Provider、同套餐最近最多 5 个未恢复失效账号的平均寿命。恢复后撤销样本；普通 Token 到期、限流和额度耗尽不计死亡。已超过平均寿命的账号跳过；全部超出或其余账号资料缺失时保持未知，不从剩余额度扣除。</p>
          <p>每分钟消耗为最近 60 秒已完成推理请求的已记录 USD，按请求授权分组范围归属，跨组可能重叠。</p>
          <p>可支撑时间按共享账号在所有分组的消耗 {{ monitorMoney(snapshot?.quotaConsumeUsdPerMinute, 'unknown', 4) }} /分计算。</p>
          <p>并发为本组 API Key 的当前占用 /（本组占用 + 可调度账号的共享空位）。其他组占用共享账号时，本组可用上限随之减少。Key 绑定多个组时占用可能重叠，不同分组的额度及并发不可直接相加。</p>
        </div>
      </BasePopover>
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
        <dd class="monitor-value" :title="metric.value">
          {{ loading && !snapshot ? '读取中' : metric.value }}
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
