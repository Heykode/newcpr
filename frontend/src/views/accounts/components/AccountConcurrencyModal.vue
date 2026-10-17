<script setup lang="ts">
import type { AccountConcurrencyPage, AccountConcurrencyPool } from '@/api/modules/accounts'
import { RefreshCw } from '@lucide/vue'
import { useNow } from '@vueuse/core'
import { onScopeDispose, ref, shallowRef, watch } from 'vue'
import { getAccountConcurrency } from '@/api/modules/accounts'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'

const props = defineProps<{ accountId: string }>()
const open = defineModel<boolean>({ default: false })
const pool = ref<AccountConcurrencyPool>('shared')
const page = ref(1)
const pageSize = ref(20)
const data = shallowRef<AccountConcurrencyPage | null>(null)
const loading = ref(false)
const error = ref('')
const now = useNow({ interval: 1000 })
const options = [{ label: '普通请求', value: 'shared' }, { label: 'Guardian 预留', value: 'reserved' }, { label: '质量检测', value: 'quality' }]
let controller: AbortController | undefined
let generation = 0
async function load() {
  controller?.abort()
  const current = ++generation
  data.value = null
  error.value = ''
  loading.value = false
  if (!open.value)
    return
  controller = new AbortController()
  loading.value = true
  try {
    const result = await getAccountConcurrency({ accountId: props.accountId, pool: pool.value, page: page.value, pageSize: pageSize.value }, { signal: controller.signal })
    if (current !== generation)
      return
    if (result && page.value > Math.max(1, Math.ceil(result.total / pageSize.value))) {
      page.value = Math.max(1, Math.ceil(result.total / pageSize.value))
      return
    }
    data.value = result
  }
  catch {
    if (current === generation)
      error.value = '并发详情读取失败'
  }
  finally {
    if (current === generation)
      loading.value = false
  }
}
watch([pool, () => props.accountId, pageSize], () => {
  page.value = 1
}, { flush: 'sync' })
watch([open, pool, page, pageSize, () => props.accountId], load, { immediate: true })
onScopeDispose(() => {
  ++generation
  controller?.abort()
})
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
      <BaseIconButton label="刷新并发详情" :disabled="loading" @click="load">
        <RefreshCw :size="16" />
      </BaseIconButton>
    </div>
    <p v-if="loading" role="status">
      加载中
    </p>
    <p v-else-if="error" role="alert">
      {{ error }}
    </p>
    <p v-else-if="!data" role="status">
      实时并发数据暂不可用
    </p>
    <template v-else>
      <div class="max-h-96 overflow-auto">
        <table class="w-full text-left text-cp-sm">
          <thead><tr><th>请求 / Key ID</th><th>模型 / IP</th><th>开始时间 / 已占用</th></tr></thead>
          <tbody>
            <tr v-for="(item, index) in data.items" :key="index" class="border-b border-cp-border">
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
      </div>
      <BaseTablePagination :pagination="{ currentPage: page, pageSize, total: data.total }" :loading="loading" @page-change="page = $event" @page-size-change="pageSize = $event" />
    </template>
  </BaseModal>
</template>
