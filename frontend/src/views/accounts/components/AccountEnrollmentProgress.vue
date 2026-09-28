<script setup lang="ts">
import type { ReloginEntry } from '@/api/modules/relogin'
import { onScopeDispose, shallowRef, watch } from 'vue'
import { getRelogin } from '@/api/modules/relogin'
import BaseButton from '@/components/base/BaseButton.vue'
import { errorMessage } from '@/utils/async'

const props = defineProps<{ ids: string[] }>()
const emit = defineEmits<{ completed: [], dismiss: [] }>()
const rows = shallowRef<ReloginEntry[]>([])
const failure = shallowRef('')
let timer: ReturnType<typeof setTimeout> | undefined
let version = 0
let controller: AbortController | undefined
const completed = new Set<string>()

function stop() {
  version++
  clearTimeout(timer)
  controller?.abort()
}

async function refresh(run: number) {
  controller = new AbortController()
  try {
    const data = await getRelogin({ silent: true, signal: controller.signal })
    if (run !== version)
      return
    rows.value = data.items.filter(row => props.ids.includes(row.id))
    failure.value = data.settings.paused ? '重登队列已暂停，恢复后继续执行' : ''
    let changed = false
    for (const row of rows.value) {
      if (row.poolStatus === 'synced' && !completed.has(row.id)) {
        completed.add(row.id)
        changed = true
      }
    }
    if (changed)
      emit('completed')
    if (rows.value.some(row => ['pending', 'queued', 'running', 'pushing'].includes(row.status) || (row.status === 'ready' && row.enrollmentPending)))
      timer = setTimeout(() => void refresh(run), 2000)
  }
  catch (error) {
    if (run !== version)
      return
    failure.value = errorMessage(error, '导入进度读取失败')
    timer = setTimeout(() => void refresh(run), 5000)
  }
}

watch(() => props.ids, () => {
  stop()
  if (props.ids.length)
    void refresh(version)
}, { immediate: true })
onScopeDispose(stop)
</script>

<template>
  <section v-if="ids.length" class="shrink-0 border-y border-cp-border py-3" aria-label="2FA导入进度">
    <div class="flex items-center justify-between gap-3">
      <span class="text-cp-sm font-medium">2FA导入 · {{ rows.filter(row => row.poolStatus === 'synced').length }}/{{ ids.length }} 已入池</span>
      <div class="flex items-center gap-3">
        <RouterLink to="/relogin" class="text-cp-sm text-cp-primary-text">
          查看失效重登
        </RouterLink>
        <BaseButton size="sm" @click="emit('dismiss')">
          收起
        </BaseButton>
      </div>
    </div>
    <p v-if="failure" class="text-cp-sm text-cp-error" role="alert">
      {{ failure }}
    </p>
    <div class="mt-2 max-h-36 overflow-y-auto">
      <div v-for="row in rows" :key="row.id" class="flex flex-wrap justify-between gap-x-4 gap-y-1 py-1 text-cp-sm">
        <span class="break-all">{{ row.email }}</span>
        <span :class="row.status === 'failed' || row.status === 'uncertain' ? 'text-cp-error' : 'text-cp-text-secondary'">{{ row.message || row.status }}</span>
      </div>
    </div>
  </section>
</template>
