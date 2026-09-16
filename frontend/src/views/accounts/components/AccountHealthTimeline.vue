<script setup lang="ts">
import type { AccountHealthBucket } from '@/api'
import { computed, shallowRef } from 'vue'
import BasePopover from '@/components/base/BasePopover.vue'

const props = withDefaults(defineProps<{
  buckets?: AccountHealthBucket[]
}>(), {
  buckets: () => [],
})

const activeBucketKey = shallowRef<string | null>(null)
const visibleBuckets = computed(() => props.buckets.slice(-6).map((bucket) => {
  const sampleCount = bucket.successCount + bucket.errorCount + bucket.nonCompletionCount
  const successRate = sampleCount > 0 ? bucket.successCount * 100 / sampleCount : null
  // Truncate the display only, so below-threshold rates never round up across it.
  const successRateDisplay = successRate === null
    ? '暂无样本'
    : `${Math.floor(bucket.successCount * 10_000 / sampleCount) / 100}%`
  return {
    ...bucket,
    sampleCount,
    successRateDisplay,
    presentation: bucketPresentation(bucket, successRate),
  }
}))

// Appearance adapted from qianxing-modelhealthcheck; see /third-party-notices.txt.
// Counts cannot identify latency, validation errors or maintenance.
function bucketPresentation(bucket: AccountHealthBucket, successRate: number | null) {
  if (successRate === null) {
    return {
      color: 'bg-slate-400',
      label: bucket.requestCount === 0 && bucket.inFlightCount === 0 ? '无请求' : '等待结果',
    }
  }
  if (successRate >= 95)
    return { color: 'bg-emerald-500', label: '正常' }
  if (successRate >= 80)
    return { color: 'bg-amber-400', label: '部分失败' }
  return { color: 'bg-red-500', label: '成功率偏低' }
}

function setBucketOpen(key: string, open: boolean) {
  if (open)
    activeBucketKey.value = key
  else if (activeBucketKey.value === key)
    activeBucketKey.value = null
}

function bucketRange(bucket: AccountHealthBucket) {
  const start = new Date(bucket.startAt)
  const end = new Date(start.getTime() + 5 * 60 * 1_000)
  return `${formatTime(start)}–${formatTime(end)}`
}

function formatTime(value: Date) {
  return new Intl.DateTimeFormat('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  }).format(value)
}
</script>

<template>
  <div
    class="account-health-timeline w-[74px] max-w-full space-y-1"
    role="group"
    aria-label="最近 30 分钟账号健康状态"
  >
    <div class="health-track relative h-8 w-full overflow-hidden rounded-[6px]">
      <div class="flex h-full w-full gap-[2px] p-[2px]">
        <BasePopover
          v-for="bucket in visibleBuckets"
          :key="bucket.key || bucket.startAt"
          :model-value="activeBucketKey === (bucket.key || bucket.startAt)"
          class="h-full min-w-0 flex-1"
          placement="top"
          trigger="hover-click"
          :hover-delay="100"
          @update:model-value="setBucketOpen(bucket.key || bucket.startAt, $event)"
        >
          <template #trigger="{ open }">
            <button
              type="button"
              class="relative block h-full w-full flex-1 cursor-pointer rounded-[1px] border-0 p-0 outline-none transition-all duration-200 hover:scale-y-110 hover:opacity-80 focus-visible:ring-2 focus-visible:ring-cp-control-outline motion-reduce:transition-none"
              :class="[bucket.presentation.color, open ? 'z-10 scale-y-110 ring-1 ring-cp-text/20' : '']"
              :aria-label="`${bucketRange(bucket)}，${bucket.presentation.label}，成功率 ${bucket.successRateDisplay}，已结束请求 ${bucket.sampleCount} 次，进行中 ${bucket.inFlightCount} 次`"
              :aria-expanded="open"
            />
          </template>
          <section class="w-56 rounded-cp-lg p-3 text-cp-sm">
            <div class="flex items-center justify-between gap-2 border-b border-cp-border-secondary pb-2">
              <span class="inline-flex items-center gap-1.5 font-heavy text-cp-text">
                <span class="size-1.5 shrink-0 rounded-full" :class="bucket.presentation.color" aria-hidden="true" />
                {{ bucket.presentation.label }}
              </span>
              <span class="font-mono text-[10px] text-cp-text-tertiary">{{ bucketRange(bucket) }}</span>
            </div>
            <dl class="mt-2 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-cp-xs">
              <dt class="text-cp-text-tertiary">
                成功率
              </dt>
              <dd class="m-0 text-right font-mono tabular-nums text-cp-text">
                {{ bucket.successRateDisplay }}
              </dd>
              <dt class="text-cp-text-tertiary" title="成功 + 失败 + 未完成，不含进行中">
                已结束请求数
              </dt>
              <dd class="m-0 text-right font-mono tabular-nums text-cp-text">
                {{ bucket.sampleCount }}
              </dd>
              <dt class="text-cp-text-tertiary">
                成功
              </dt>
              <dd class="m-0 text-right font-mono tabular-nums text-cp-success-text">
                {{ bucket.successCount }}
              </dd>
              <dt class="text-cp-text-tertiary">
                失败
              </dt>
              <dd class="m-0 text-right font-mono tabular-nums text-cp-error-text">
                {{ bucket.errorCount }}
              </dd>
              <dt class="text-cp-text-tertiary" title="取消或未正常完成的请求，不计入失败">
                未完成
              </dt>
              <dd class="m-0 text-right font-mono tabular-nums text-cp-orange-text">
                {{ bucket.nonCompletionCount }}
              </dd>
              <dt class="text-cp-text-tertiary">
                进行中
              </dt>
              <dd class="m-0 text-right font-mono tabular-nums text-cp-info-text">
                {{ bucket.inFlightCount }}
              </dd>
            </dl>
          </section>
        </BasePopover>
      </div>
    </div>
    <div class="flex justify-between text-[9px] leading-3 text-cp-text-tertiary" aria-hidden="true">
      <span>较早</span>
      <span>现在</span>
    </div>
  </div>
</template>

<style scoped>
.account-health-timeline {
  --health-muted: oklch(0.97 0 0);
}

[data-theme='dark'] .account-health-timeline {
  --health-muted: oklch(0.269 0 0);
}

.health-track {
  background-color: color-mix(in oklab, var(--health-muted) 20%, transparent);
}
</style>
