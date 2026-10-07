import type { Plugin } from 'vue'

import { setSessionRecoveryHandler, setUnauthorizedHandler } from '@/api/request'
import { router } from '@/router'
import { pinia } from '@/stores'
import { useAuthStore } from '@/stores/modules/auth'

export const authPlugin: Plugin = {
  install(app) {
    const authStore = useAuthStore(pinia)

    setUnauthorizedHandler(async () => {
      authStore.invalidateSession()
      if (router.currentRoute.value.path !== '/login')
        await router.replace({ name: 'login' })
    })
    setSessionRecoveryHandler(async () => {
      const result = await authStore.checkAuth()
      if (result === 'unavailable')
        throw new Error(authStore.sessionCheckError)
      return result === 'authenticated'
    })
    let lastAttempt = 0
    async function restore(force = false) {
      if (document.visibilityState !== 'visible' || authStore.loading || !authStore.isAuthenticated)
        return
      if (!force && Date.now() - lastAttempt < 60_000)
        return
      lastAttempt = Date.now()
      const result = await authStore.checkAuth()
      if (result === 'unauthenticated' && !authStore.loading) {
        authStore.invalidateSession()
        await router.replace({ name: 'login' })
      }
    }
    const activity = () => {
      void restore().catch(() => {})
    }
    const resume = () => {
      void restore(true).catch(() => {})
    }
    document.addEventListener('pointerdown', activity, { passive: true })
    document.addEventListener('keydown', activity)
    document.addEventListener('scroll', activity, { passive: true, capture: true })
    document.addEventListener('visibilitychange', resume)
    window.addEventListener('focus', resume)
    window.addEventListener('online', resume)
    app.onUnmount(() => {
      document.removeEventListener('pointerdown', activity)
      document.removeEventListener('keydown', activity)
      document.removeEventListener('scroll', activity, true)
      document.removeEventListener('visibilitychange', resume)
      window.removeEventListener('focus', resume)
      window.removeEventListener('online', resume)
    })
  },
}
