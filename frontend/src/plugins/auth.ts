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
      const result = await authStore.refreshSession()
      if (result === 'unavailable')
        throw new Error(authStore.sessionCheckError)
      return result === 'authenticated'
    })
    async function restore() {
      if (document.visibilityState !== 'visible' || authStore.loading || authStore.sessionChecked)
        return
      await router.replace(router.currentRoute.value.fullPath)
    }
    const resume = () => {
      void restore().catch(() => {})
    }
    document.addEventListener('visibilitychange', resume)
    window.addEventListener('focus', resume)
    window.addEventListener('online', resume)
    app.onUnmount(() => {
      document.removeEventListener('visibilitychange', resume)
      window.removeEventListener('focus', resume)
      window.removeEventListener('online', resume)
    })
  },
}
