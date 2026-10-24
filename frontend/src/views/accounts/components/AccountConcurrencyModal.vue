<script setup lang="ts">
import type { AccountConcurrencyPool } from '@/api/modules/accounts'
import { useIntervalFn, useNow } from '@vueuse/core'
import { ref } from 'vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'
import { t } from '@/i18n'
import { useAccountConcurrency } from '../composables/useAccountConcurrency'

const props = defineProps<{ accountId: string }>()
const open = defineModel<boolean>({ default: false })
const pool = ref<AccountConcurrencyPool>('shared')
const { data, loading, error } = useAccountConcurrency(() => props.accountId, open, pool)
const now = useNow({ scheduler: callback => useIntervalFn(callback, 1000) })
const options = [{ label: '普通请求', value: 'shared' }, { label: 'Guardian 预留', value: 'reserved' }, { label: '质量检测', value: 'quality' }]
function time(value: number) {
  return value > 0 ? new Date(value).toLocaleString() : '未知'
}
function duration(value: number) {
  return value > 0 ? `${Math.max(0, Math.floor((now.value.getTime() - value) / 1000))} 秒` : '未知'
}
</script>

<template>
  <BaseModal v-model="open" title="账号实时并发" size="lg">
    <div class="flex flex-wrap items-center justify-between gap-2">
      <BaseSegmented v-model="pool" label="并发池" :options="options" />
    </div>
    <div class="h-96 max-h-[60dvh] overflow-auto" :aria-busy="loading">
      <p v-if="loading && !data" role="status">
        {{ t('common.status.loading') }}
      </p>
      <p v-if="error" role="alert">
        {{ error }}
      </p>
      <p v-else-if="!data && !loading" role="status">
        实时并发数据暂不可用
      </p>
      <template v-if="data">
        <table class="w-full text-left text-cp-sm">
          <thead><tr><th>请求 / Key ID</th><th>模型 / IP</th><th>开始时间 / 已占用</th></tr></thead>
          <tbody>
            <tr v-for="(item, index) in data.items" :key="item.leaseId || index" class="border-b border-cp-border">
              <td class="max-w-64 py-2 break-all">
                {{ item.detail?.requestId || '旧租约（无详情）' }}<div class="text-cp-xs text-cp-text-secondary">
                  {{ item.detail?.clientKeyId || '未知' }}
                </div>
              </td>
              <td class="max-w-48 break-all">
                {{ item.detail?.model || '未知' }}<div class="text-cp-xs text-cp-text-secondary">
                  {{ item.detail?.clientIp || '未知' }}
                </div>
              </td>
              <td>
                {{ time(item.detail?.startedAtMs || 0) }}<div class="text-cp-xs text-cp-text-secondary">
                  {{ duration(item.detail?.startedAtMs || 0) }}
                </div>
              </td>
            </tr>
          </tbody>
        </table>
        <p v-if="!data.items.length">
          当前没有执行中的请求
        </p>
      </template>
    </div>
  </BaseModal>
</template>
