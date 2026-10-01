<script setup lang="ts">
import type { Account } from '@/api'
import type { ResetBatch, ResetItemStatus } from '@/api/modules/reset-credits'
import { computed } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'

const props = defineProps<{
  batch: ResetBatch | null
  batches: ResetBatch[]
  accounts: Account[]
  busy: boolean
  error: string
  typeOptions: { label: string, value: string }[]
}>()
const emit = defineEmits<{ confirm: [], prepare: [], retry: [accountId: string] }>()
const open = defineModel<boolean>({ required: true })
const resetType = defineModel<string>('resetType', { required: true })
const selectedBatchId = defineModel<string>('selectedBatchId', { required: true })
const readyCount = computed(() => props.batch?.items.filter(i => i.status === 'ready').length ?? 0)
const doneCount = computed(() => props.batch?.items.filter(i => !['ready', 'queued', 'running'].includes(i.status)).length ?? 0)
const labels: Record<ResetItemStatus, string> = {
  ready: '待确认',
  queued: '排队中',
  running: '处理中',
  succeeded: '成功',
  skipped: '跳过',
  failed: '失败',
  unknown: '待确认结果',
}
const batchOptions = computed(() => props.batches.map(batch => ({
  value: batch.id,
  label: `${date(batch.createdAt)} · ${batch.items.length} 个账号`,
})))
function accountLabel(id: string) {
  const account = props.accounts.find(a => a.id === id)
  return account?.customName || account?.email || id
}
function date(value: string | null) {
  if (!value)
    return '未知'
  return new Date(value).toLocaleString()
}
</script>

<template>
  <BaseModal
    v-model="open"
    :title="batch && !batch.confirmed ? '确认使用重置次数' : '批量重置记录'"
    size="xl"
    :dismissible="!busy"
  >
    <div class="grid min-w-0 gap-4">
      <p v-if="error" role="alert" class="m-0 break-words text-cp-error-text">
        {{ error }}
      </p>
      <div v-if="batch && !batch.confirmed" class="grid min-w-0 items-end gap-3 sm:grid-cols-[minmax(0,1fr)_auto]">
        <div class="grid min-w-0 gap-1 text-cp-sm text-cp-text-secondary">
          <span>重置类型</span>
          <BaseSelect v-model="resetType" class="w-full min-w-0" :options="typeOptions" :disabled="busy" aria-label="重置类型" @update:model-value="emit('prepare')" />
        </div>
        <p class="m-0 text-cp-sm">
          已选 {{ batch.items.length }} 个 · 可用 {{ readyCount }} 个 · 跳过 {{ batch.items.length - readyCount }} 个
        </p>
      </div>
      <BaseSelect v-else-if="batches.length" v-model="selectedBatchId" :options="batchOptions" aria-label="重置任务" :disabled="busy" />
      <p v-if="batch && !batch.confirmed" class="m-0 text-cp-warning-text">
        本次最多消耗 {{ readyCount }} 次，每个账号一张，优先最早到期。重置后不能撤销。
      </p>
      <p v-else-if="batch" class="m-0 text-cp-text-secondary">
        已处理 {{ doneCount }} / {{ batch.items.length }}
      </p>
      <div v-if="batch" class="divide-y divide-cp-border border-y border-cp-border">
        <article v-for="item in batch.items" :key="item.accountId" class="grid min-w-0 gap-2 py-3 sm:grid-cols-[minmax(0,1.2fr)_minmax(0,1fr)_minmax(0,1fr)] sm:gap-4">
          <div class="min-w-0">
            <div class="break-all font-medium">
              {{ accountLabel(item.accountId) }}
            </div>
            <div class="mt-1 text-cp-sm text-cp-text-secondary">
              可用次数：{{ item.availableCount ?? '未知' }}
            </div>
          </div>
          <div class="min-w-0 break-words text-cp-sm text-cp-text-secondary">
            <div>{{ item.credit?.title || item.credit?.resetType || '未选卡' }}</div>
            <div v-if="item.credit" class="mt-1">
              到期：{{ date(item.credit.expiresAt) }}
            </div>
          </div>
          <div class="min-w-0 break-words text-cp-sm">
            <div :class="item.status === 'succeeded' ? 'text-cp-success-text' : item.status === 'unknown' || item.status === 'failed' ? 'text-cp-warning-text' : 'text-cp-text-secondary'">
              {{ labels[item.status] }}
            </div>
            <div class="mt-1 text-cp-text-secondary">
              {{ item.message }}
            </div>
            <BaseButton v-if="item.status === 'unknown'" class="mt-2" variant="secondary" :disabled="busy" @click="emit('retry', item.accountId)">
              继续确认原操作
            </BaseButton>
          </div>
        </article>
      </div>
      <p v-else class="m-0 py-6 text-center text-cp-text-secondary">
        {{ busy ? '正在查询所选账号的重置次数…' : '暂无批量重置记录' }}
      </p>
    </div>
    <template #footer>
      <BaseButton variant="secondary" :disabled="busy" @click="open = false">
        {{ batch && !batch.confirmed ? '取消' : '关闭' }}
      </BaseButton>
      <BaseButton v-if="batch && !batch.confirmed" variant="primary" :loading="busy" :disabled="busy || !readyCount" @click="emit('confirm')">
        确认使用 {{ readyCount }} 次
      </BaseButton>
    </template>
  </BaseModal>
</template>
