<script setup lang="ts">
import { RefreshCw } from '@lucide/vue'
import { useEventListener } from '@vueuse/core'
import { onMounted, onUnmounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'

import BaseButton from '@/components/base/BaseButton.vue'
import { useAuthStore } from '@/stores/modules/auth'

const auth = useAuthStore()
const route = useRoute()
const router = useRouter()
const retrying = ref(false)
const retryDelays = [1000, 3000]
let automaticRetries = 0
let timer: ReturnType<typeof setTimeout> | undefined
let active = true

function clearRetry() {
  clearTimeout(timer)
  timer = undefined
}

function scheduleRetry() {
  clearRetry()
  if (active && automaticRetries < retryDelays.length) {
    const delay = retryDelays[automaticRetries++]!
    timer = setTimeout(() => void retry(), delay)
  }
}

function returnPath() {
  const value = route.query.redirect
  if (typeof value !== 'string' || !value.startsWith('/') || value.startsWith('//'))
    return '/'
  if (value.includes('\\') || Array.from(value).some(char => char.charCodeAt(0) < 32 || char.charCodeAt(0) === 127))
    return '/'
  const path = value.split(/[?#]/)[0]?.replace(/\/+$/, '').toLowerCase()
  return path === '/session-recovery' || path === '/login' ? '/' : value
}

async function retry() {
  if (!active || retrying.value)
    return
  clearRetry()
  retrying.value = true
  try {
    const result = await auth.checkAuth()
    if (!active)
      return
    if (result === 'authenticated')
      await router.replace(returnPath())
    else if (result === 'unauthenticated')
      await router.replace('/login')
    else
      scheduleRetry()
  }
  catch {
    // A route chunk can still be unavailable while the connection is recovering.
    scheduleRetry()
  }
  finally {
    retrying.value = false
  }
}

useEventListener(window, 'online', () => void retry())
useEventListener(document, 'visibilitychange', () => {
  if (document.visibilityState === 'visible')
    void retry()
})
onMounted(scheduleRetry)
onUnmounted(() => {
  active = false
  clearRetry()
})
</script>

<template>
  <main class="grid min-h-dvh place-items-center bg-cp-bg-layout px-6 text-cp-text">
    <section class="w-full max-w-md text-center" aria-labelledby="session-recovery-title">
      <RefreshCw class="mx-auto mb-4 size-8 text-cp-text-secondary" :class="{ 'animate-spin': retrying }" aria-hidden="true" />
      <h1 id="session-recovery-title" class="text-xl font-semibold">
        正在恢复连接
      </h1>
      <p class="mt-3 text-sm text-cp-text-secondary" role="status" aria-live="polite">
        {{ auth.sessionCheckError || '暂时无法确认登录状态，请稍后重试' }}
      </p>
      <BaseButton class="mt-6" variant="primary" :loading="retrying" :disabled="retrying" @click="retry">
        <template #icon>
          <RefreshCw aria-hidden="true" />
        </template>
        重新连接
      </BaseButton>
    </section>
  </main>
</template>
