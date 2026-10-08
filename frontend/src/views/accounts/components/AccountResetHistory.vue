<script setup lang="ts">
import type { ResetHistoryPage, ResetItemStatus } from '@/api/modules/reset-credits'
import { ChevronLeft, ChevronRight, RefreshCw, Search } from '@lucide/vue'
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { getResetHistory, retryResetBatch } from '@/api/modules/reset-credits'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import { errorMessage } from '@/utils/async'

const result = ref<ResetHistoryPage | null>(null)
const page = ref(1)
const search = ref('')
const query = ref('')
const loading = ref(false)
const retrying = ref(false)
const error = ref('')
let before: string | undefined
let alive = true
let generation = 0
let timer: ReturnType<typeof setTimeout> | undefined
let controller: AbortController | undefined
const labels: Record<ResetItemStatus, string> = {
  ready: '待确认',
  queued: '排队中',
  running: '处理中',
  succeeded: '成功',
  skipped: '跳过',
  failed: '失败',
  unknown: '待确认结果',
}
async function load(target = page.value, reset = false) {
  const version = ++generation
  controller?.abort()
  controller = new AbortController()
  clearTimeout(timer)
  loading.value = true
  error.value = ''
  try {
    const next = await getResetHistory({ page: target, search: query.value, before: reset ? undefined : before }, { signal: controller.signal, silent: true })
    if (!alive || version !== generation)
      return
    result.value = next
    before = next.before
    page.value = target
  }
  catch (cause) {
    if (alive && version === generation)
      error.value = errorMessage(cause, '重置记录读取失败')
  }
  finally {
    if (alive && version === generation) {
      loading.value = false
      if (result.value?.items.some(batch => batch.items.some(item => ['queued', 'running'].includes(item.status))))
        timer = setTimeout(() => void load(), 3000)
    }
  }
}
function filter() {
  query.value = search.value.trim()
  result.value = null
  page.value = 1
  before = undefined
  void load(1, true)
}
async function retry(batchId: string, accountId: string) {
  if (retrying.value)
    return
  retrying.value = true
  try {
    await retryResetBatch(batchId, accountId)
    if (alive)
      await load()
  }
  catch (cause) {
    if (alive)
      error.value = errorMessage(cause, '原操作结果尚未确认')
  }
  finally { retrying.value = false }
}
onMounted(() => void load(1, true))
onBeforeUnmount(() => {
  alive = false
  generation++
  controller?.abort()
  clearTimeout(timer)
})
</script>

<template>
  <div class="grid min-w-0 gap-3">
    <form class="flex min-w-0 items-center gap-2" @submit.prevent="filter">
      <BaseInput v-model="search" aria-label="搜索重置记录账号" placeholder="账号名称、邮箱或 ID" :maxlength="200" />
      <BaseIconButton type="submit" label="搜索账号" :disabled="loading">
        <Search class="size-4" />
      </BaseIconButton>
      <BaseIconButton label="刷新重置记录" :disabled="loading" @click="load(1, true)">
        <RefreshCw class="size-4" />
      </BaseIconButton>
    </form>
    <p v-if="error" role="alert" class="m-0 break-words text-cp-error-text">
      {{ error }}
    </p>
    <p v-if="loading" role="status" class="m-0 text-cp-text-secondary">
      读取中…
    </p>
    <p v-else-if="!result?.items.length && !error" class="m-0 py-6 text-center text-cp-text-secondary">
      暂无匹配的重置记录
    </p>
    <section v-for="batch in result?.items ?? []" :key="batch.id" class="min-w-0 border-t border-cp-border" :aria-label="`重置批次 ${batch.id}`">
      <header class="flex flex-wrap justify-between gap-2 py-3 text-cp-sm">
        <span>{{ new Date(batch.createdAt).toLocaleString() }}</span>
        <span class="text-cp-text-secondary">{{ batch.items.length }} 个账号 · 已处理 {{ batch.items.filter(item => !['ready', 'queued', 'running'].includes(item.status)).length }}</span>
      </header>
      <div class="divide-y divide-cp-border">
        <article v-for="item in batch.items" :key="item.accountId" class="grid min-w-0 gap-2 py-3 text-cp-sm sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
          <div class="min-w-0">
            <div class="break-all font-medium">
              {{ result?.accountNames[item.accountId] || item.accountId }}
            </div>
            <div class="mt-1 break-words text-cp-text-secondary">
              {{ item.credit?.title || item.credit?.resetType || '未选卡' }} · 可用次数 {{ item.availableCount ?? '未知' }}
            </div>
          </div>
          <div class="min-w-0 break-words">
            <span :class="item.status === 'succeeded' ? 'text-cp-success-text' : item.status === 'failed' || item.status === 'unknown' ? 'text-cp-warning-text' : 'text-cp-text-secondary'">{{ labels[item.status] }}</span>
            <div class="mt-1 text-cp-text-secondary">
              {{ item.message }}
            </div>
            <BaseButton v-if="item.status === 'unknown'" class="mt-2" variant="secondary" :disabled="retrying" @click="retry(batch.id, item.accountId)">
              继续确认原操作
            </BaseButton>
          </div>
        </article>
      </div>
    </section>
    <nav class="flex items-center justify-end gap-3 border-t border-cp-border pt-3" aria-label="重置记录分页">
      <BaseIconButton label="上一页" :disabled="loading || page <= 1" @click="load(page - 1)">
        <ChevronLeft class="size-4" />
      </BaseIconButton>
      <span class="text-cp-sm tabular-nums">第 {{ page }} 页</span>
      <BaseIconButton label="下一页" :disabled="loading || !result?.hasMore" @click="load(page + 1)">
        <ChevronRight class="size-4" />
      </BaseIconButton>
    </nav>
  </div>
</template>
