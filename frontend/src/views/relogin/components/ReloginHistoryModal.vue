<script setup lang="ts">
import type { ReloginAttempt, ReloginEntry } from '@/api/modules/relogin'
import { onBeforeUnmount, shallowRef, watch } from 'vue'
import { getReloginHistory } from '@/api/modules/relogin'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import { errorMessage } from '@/utils/async'
import { formatDateTime } from '@/utils/date'

const props = defineProps<{ entry: ReloginEntry | null }>()
const emit = defineEmits<{ afterLeave: [] }>()
const open = defineModel<boolean>({ default: false })
const items = shallowRef<ReloginAttempt[]>([])
const cursor = shallowRef<string | null>(null)
const loading = shallowRef(false)
const failure = shallowRef('')
const sources = { library: '资料库重登', account_menu: '立即重登', automatic: '自动重登', enrollment: '导入登录' }
const results = { running: '登录中', succeeded: '登录成功', failed: '登录失败', cancelled: '已取消', awaiting_workspace: '待选择工作区' }
const pushes = { not_requested: '未推送', pending: '等待自动推送', running: '推送中', succeeded: '推送成功', failed: '推送失败', uncertain: '推送待核实' }
let controller: AbortController | undefined

async function load(more = false, event?: MouseEvent) {
  if (!open.value || !props.entry || (more && (loading.value || !cursor.value)))
    return
  if (event && event.currentTarget instanceof HTMLElement)
    event.currentTarget.closest<HTMLElement>('[role="dialog"]')?.focus()
  controller?.abort()
  const current = new AbortController()
  controller = current
  loading.value = true
  failure.value = ''
  try {
    const result = await getReloginHistory(props.entry.id, more ? cursor.value! : undefined, { signal: current.signal, silent: true })
    if (current.signal.aborted)
      return
    items.value = more ? [...items.value, ...result.items] : result.items
    cursor.value = result.nextCursor
  }
  catch (error) {
    if (!current.signal.aborted)
      failure.value = errorMessage(error, '重登记录读取失败')
  }
  finally {
    if (!current.signal.aborted)
      loading.value = false
  }
}
watch(() => [open.value, props.entry?.id] as const, ([visible]) => {
  controller?.abort()
  loading.value = false
  if (visible) {
    items.value = []
    cursor.value = null
    void load()
  }
}, { immediate: true, flush: 'sync' })
onBeforeUnmount(() => controller?.abort())
function afterLeave() {
  if (!open.value) {
    items.value = []
    failure.value = ''
    cursor.value = null
  }
  emit('afterLeave')
}
</script>

<template>
  <BaseModal v-model="open" title="重登详情" :description="entry?.email" size="lg" @after-leave="afterLeave">
    <p v-if="failure" role="alert" class="mb-3 text-cp-error">
      {{ failure }}
    </p>
    <p v-if="!items.length" role="status" class="text-cp-text-tertiary">
      {{ loading ? '正在读取' : failure ? '记录暂不可用' : '暂无逐次记录，旧版重登详情未保存' }}
    </p>
    <ol class="space-y-3">
      <li v-for="item in items" :key="item.id" class="rounded-cp-lg border border-cp-border p-3">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <time class="font-medium" :datetime="item.startedAt">{{ formatDateTime(item.startedAt) }}</time>
          <span :class="item.result === 'succeeded' ? 'text-cp-success' : item.result === 'failed' ? 'text-cp-error' : 'text-cp-text-secondary'">{{ results[item.result] }}</span>
        </div>
        <p class="mt-1 text-cp-xs text-cp-text-tertiary">
          {{ item.credentialTarget === 'excel' ? 'Excel' : 'Codex' }} · {{ sources[item.source] }}<span v-if="item.finishedAt"> · 完成 {{ formatDateTime(item.finishedAt) }}</span>
        </p>
        <p v-if="item.message" class="mt-2 whitespace-pre-wrap break-words text-cp-sm">
          {{ item.message }}
        </p>
        <p v-if="item.workspaceId" class="mt-1 break-all text-cp-xs text-cp-text-tertiary">
          工作区 {{ item.workspaceId }}
        </p>
        <div class="mt-2 border-t border-cp-border pt-2 text-cp-sm">
          <span :class="item.pushResult === 'succeeded' ? 'text-cp-success' : item.pushResult === 'failed' || item.pushResult === 'uncertain' ? 'text-cp-error' : 'text-cp-text-secondary'">{{ pushes[item.pushResult] }}</span>
          <span v-if="item.pushedAt" class="ml-2 text-cp-text-tertiary">{{ formatDateTime(item.pushedAt) }}</span>
          <p v-if="item.pushMessage" class="mt-1 whitespace-pre-wrap break-words">
            {{ item.pushMessage }}
          </p>
        </div>
      </li>
    </ol>
    <template #footer>
      <BaseButton :disabled="loading" @click="load(false, $event)">
        刷新
      </BaseButton>
      <BaseButton v-if="cursor" :loading="loading" :disabled="loading" @click="load(true, $event)">
        加载更多
      </BaseButton>
    </template>
  </BaseModal>
</template>
