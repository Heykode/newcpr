<script setup lang="ts">
import { RefreshCw, Save, Square, Trash2 } from '@lucide/vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseNumberInput from '@/components/base/BaseNumberInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import { useLogCleanup } from '../../composables/useLogCleanup'
import { cleanupBytes, cleanupCategories, cleanupCutoff } from './cleanup'
import CleanupRetentionInput from './CleanupRetentionInput.vue'

const { state, draft, usage, preview, confirming, busy, refreshing, loading, error, usageError, dirty, running, selected, refreshUsage, save, requestCleanup, confirmCleanup, stop } = useLogCleanup()
const frequencies = [
  { label: '每小时', value: 'hourly' },
  { label: '每6小时', value: 'six_hourly' },
  { label: '每天指定时间', value: 'daily' },
]
const zones = [
  { label: '中国标准时间（UTC+8）', value: 'Asia/Shanghai' },
  { label: 'UTC', value: 'UTC' },
  { label: '洛杉矶时间', value: 'America/Los_Angeles' },
]
function date(value: string) {
  return new Date(value).toLocaleString('zh-CN', { timeZone: draft.value?.timezone || 'Asia/Shanghai', hour12: false })
}
</script>

<template>
  <section class="min-w-0" aria-label="日志清理">
    <div class="flex flex-wrap items-center justify-between gap-3 border-b border-cp-border pb-4">
      <div>
        <h2 class="m-0 text-base font-semibold">
          日志清理
        </h2>
        <p class="mt-1 mb-0 text-cp-xs text-cp-text-secondary">
          {{ usage ? `占用更新于 ${date(usage.measuredAt)}` : '当前占用尚未读取' }}
        </p>
      </div>
      <div class="flex flex-wrap items-center gap-2">
        <BaseIconButton label="刷新当前占用" :disabled="refreshing" @click="refreshUsage">
          <RefreshCw class="size-4" :class="{ 'animate-spin': refreshing }" />
        </BaseIconButton>
        <BaseButton :disabled="!draft || loading || busy || running || !dirty" :loading="busy" @click="save">
          <template #icon>
            <Save class="size-4" />
          </template>保存设置
        </BaseButton>
        <BaseButton variant="primary" :disabled="!draft || busy || loading || dirty || running || !selected" @click="requestCleanup">
          <template #icon>
            <Trash2 class="size-4" />
          </template>立即清理
        </BaseButton>
      </div>
    </div>
    <p v-if="error" role="alert" class="text-cp-danger">
      {{ error }}
    </p>
    <p v-if="usageError" role="alert" class="text-cp-danger">
      占用读取失败：{{ usageError }}
    </p>
    <p v-if="loading && !draft" role="status" class="py-6 text-cp-text-secondary">
      正在读取清理设置...
    </p>
    <template v-if="draft">
      <div class="cleanup-grid border-b border-cp-border py-3 text-cp-xs text-cp-text-secondary" aria-hidden="true">
        <span>清理项目</span><span>当前占用</span><span>保留最近</span>
      </div>
      <div v-for="item in cleanupCategories" :key="item.key" class="cleanup-grid border-b border-cp-border py-4">
        <BaseCheckbox v-model="draft[item.key].selected" :label="item.label" show-label :disabled="busy || running" />
        <span class="font-mono text-sm tabular-nums" :aria-label="`${item.label}当前占用`">
          {{ refreshing ? '读取中...' : usage ? cleanupBytes(usage.items.find(row => row.category === item.key)?.bytes) : '读取失败' }}
        </span>
        <CleanupRetentionInput v-model="draft[item.key].retentionDays" :label="item.label" :max="item.max" :disabled="busy || running" />
      </div>
      <div class="flex flex-wrap items-center gap-x-6 gap-y-4 border-b border-cp-border py-5">
        <BaseSwitch v-model="draft.enabled" label="启用自动清理" show-label :disabled="busy || running" />
        <div class="grid min-w-0 gap-1.5">
          <span class="text-cp-xs text-cp-text-secondary">执行频率</span>
          <BaseSelect v-model="draft.frequency" :options="frequencies" :disabled="busy || running || !draft.enabled" aria-label="执行频率" class="w-44 max-w-full" />
        </div>
        <div v-if="draft.frequency === 'daily'" class="grid gap-1.5">
          <span class="text-cp-xs text-cp-text-secondary">执行时间</span>
          <div class="flex flex-wrap gap-2">
            <BaseNumberInput v-model="draft.dailyHour" label="执行小时" unit="时" :min="0" :max="23" :disabled="busy || running || !draft.enabled" />
            <BaseNumberInput v-model="draft.dailyMinute" label="执行分钟" unit="分" :min="0" :max="59" :disabled="busy || running || !draft.enabled" />
          </div>
        </div>
        <div class="grid min-w-0 gap-1.5">
          <span class="text-cp-xs text-cp-text-secondary">时区</span>
          <BaseSelect v-model="draft.timezone" :options="zones" :disabled="busy || running" aria-label="清理时区" class="w-60 max-w-full" />
        </div>
      </div>
      <p v-if="dirty" role="status" class="text-cp-xs text-cp-text-secondary">
        设置尚未保存
      </p>
      <p v-else-if="state?.nextRunAt && state.config.enabled" class="text-cp-xs text-cp-text-secondary">
        下次自动清理：{{ date(state.nextRunAt) }}
      </p>
      <div v-if="running && state?.job" role="status" class="my-4 flex flex-wrap items-center justify-between gap-3">
        <span class="text-sm">正在清理{{ cleanupCategories[state.job.categoryIndex]?.label || '日志' }} · 已处理 {{ state.job.removed }} 条记录或文件</span>
        <BaseButton :disabled="busy" @click="stop">
          <template #icon>
            <Square class="size-4" />
          </template>停止清理
        </BaseButton>
      </div>
      <p class="mb-1 text-cp-xs leading-6 text-cp-text-secondary">
        全部清理仅包含已结束历史。请求日志含诊断轨迹、索引及运维事件；清理后对应历史详情和明细统计不可查，账号、凭据、余额及累计计费保留。
      </p>
      <p class="my-1 text-cp-xs leading-6 text-cp-text-secondary">
        正在运行的请求、当前活动日志文件和正在运行的采集任务不会删除。数据库释放的空间可能先供内部复用，磁盘占用不一定立即下降；日志和采集文件仅统计、清理当前服务实例。
      </p>
    </template>
    <BaseConfirmModal v-model="confirming" title="确认清理日志" confirm-text="确认清理" destructive :loading="busy" @confirm="confirmCleanup">
      <template v-if="preview">
        <ul class="m-0 grid gap-3 pl-5 text-sm">
          <template v-for="item in cleanupCategories" :key="item.key">
            <li v-if="preview.config[item.key].selected">
              {{ item.label }}：{{ preview.config[item.key].retentionDays === 0 ? '全部清理已结束历史' : `保留最近${preview.config[item.key].retentionDays}天` }}，清理 {{ date(cleanupCutoff(preview.cutoffAt, preview.config[item.key].retentionDays)) }} 之前的数据
            </li>
          </template>
        </ul>
        <p v-if="preview.config.requests.selected" class="mt-4 text-sm">
          删除请求日志会一并删除诊断轨迹和关联运维事件，历史明细统计也将减少。
        </p>
        <p class="mt-3 mb-0 text-sm text-cp-danger">
          删除不可撤销。正在进行的请求、当前日志文件和备份不会清理。
        </p>
      </template>
    </BaseConfirmModal>
  </section>
</template>

<style scoped>
.cleanup-grid {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 140px 160px;
  gap: 16px;
  align-items: center;
}
@media (max-width: 640px) {
  .cleanup-grid {
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    gap: 12px;
  }
  .cleanup-grid > :first-child {
    grid-column: 1 / -1;
  }
}
</style>
