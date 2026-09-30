import { defineStore } from 'pinia'
import { ref } from 'vue'

import { login as apiLogin, logout as apiLogout, getAuthStatus } from '@/api'
import { ApiError, resetUnauthorizedHandling } from '@/api/request'

export type AuthCheckResult = 'authenticated' | 'unauthenticated' | 'unavailable'

export const useAuthStore = defineStore('auth', () => {
  const isAuthenticated = ref(false)
  const sessionChecked = ref(false)
  const sessionCheckError = ref('')
  const loading = ref(false)
  let revision = 0
  let pendingCheck: Promise<AuthCheckResult> | undefined

  function currentResult(): AuthCheckResult {
    return isAuthenticated.value ? 'authenticated' : sessionChecked.value ? 'unauthenticated' : 'unavailable'
  }

  function invalidatePendingCheck() {
    revision += 1
    pendingCheck = undefined
    sessionCheckError.value = ''
  }

  function checkAuth(): Promise<AuthCheckResult> {
    if (pendingCheck)
      return pendingCheck
    const checkedRevision = revision
    pendingCheck = (async (): Promise<AuthCheckResult> => {
      try {
        const status = await getAuthStatus({ silent: true })
        if (checkedRevision !== revision)
          return currentResult()
        if (typeof status?.authenticated !== 'boolean')
          throw new Error('Invalid authentication status response')
        isAuthenticated.value = status.authenticated
        sessionChecked.value = true
        sessionCheckError.value = ''
        if (status.authenticated)
          resetUnauthorizedHandling()
        return currentResult()
      }
      catch (error: unknown) {
        if (checkedRevision !== revision)
          return currentResult()
        if (error instanceof ApiError && error.status === 401) {
          isAuthenticated.value = false
          sessionChecked.value = true
          sessionCheckError.value = ''
          return 'unauthenticated'
        }
        // A failed check is not evidence that the server revoked the session.
        sessionChecked.value = false
        sessionCheckError.value = '暂时无法确认登录状态，请检查网络或稍后重试'
        return 'unavailable'
      }
      finally {
        if (checkedRevision === revision)
          pendingCheck = undefined
      }
    })()
    return pendingCheck
  }

  async function login(payload: Parameters<typeof apiLogin>[0]) {
    invalidatePendingCheck()
    try {
      loading.value = true
      await apiLogin(payload)

      isAuthenticated.value = true
      sessionChecked.value = true
      resetUnauthorizedHandling()

      return true
    }
    catch {
      isAuthenticated.value = false
      return false
    }
    finally {
      loading.value = false
    }
  }

  async function logout() {
    invalidatePendingCheck()
    try {
      await apiLogout({ silent: true })
    }
    catch {
      // 忽略登出错误
    }
    finally {
      isAuthenticated.value = false
      sessionChecked.value = true
    }
  }

  function invalidateSession() {
    invalidatePendingCheck()
    isAuthenticated.value = false
    sessionChecked.value = true
    loading.value = false
  }

  return {
    isAuthenticated,
    sessionChecked,
    sessionCheckError,
    loading,
    checkAuth,
    login,
    logout,
    invalidateSession,
  }
})
