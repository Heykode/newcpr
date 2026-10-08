import { defineStore } from 'pinia'
import { ref } from 'vue'

import { login as apiLogin, logout as apiLogout, getAuthStatus, refreshAuthSession } from '@/api'
import { ApiError, invalidatePendingRequests, resetUnauthorizedHandling } from '@/api/request'

export type AuthCheckResult = 'authenticated' | 'unauthenticated' | 'unavailable'

export const useAuthStore = defineStore('auth', () => {
  const isAuthenticated = ref(false)
  const sessionChecked = ref(false)
  const sessionCheckError = ref('')
  const loading = ref(false)
  let revision = 0
  let pendingCheck: Promise<AuthCheckResult> | undefined
  let pendingRefresh: Promise<AuthCheckResult> | undefined
  let transition: Promise<void> = Promise.resolve()
  let refreshBarrier: Promise<void> = Promise.resolve()

  function currentResult(): AuthCheckResult {
    return isAuthenticated.value ? 'authenticated' : sessionChecked.value ? 'unauthenticated' : 'unavailable'
  }

  function invalidatePendingCheck() {
    revision += 1
    pendingCheck = undefined
    pendingRefresh = undefined
    sessionCheckError.value = ''
  }

  function checkAuth(): Promise<AuthCheckResult> {
    if (loading.value)
      return transition.then(currentResult)
    if (pendingRefresh)
      return pendingRefresh
    if (pendingCheck)
      return pendingCheck
    const check = loadSession(getAuthStatus).finally(() => {
      if (pendingCheck === check)
        pendingCheck = undefined
    })
    pendingCheck = check
    return check
  }

  function refreshSession(): Promise<AuthCheckResult> {
    if (loading.value)
      return transition.then(currentResult)
    if (pendingRefresh)
      return pendingRefresh
    const refresh = loadSession(refreshAuthSession).finally(() => {
      if (pendingRefresh === refresh)
        pendingRefresh = undefined
    })
    pendingRefresh = refresh
    refreshBarrier = Promise.allSettled([refreshBarrier, refresh]).then(() => {})
    return refresh
  }

  function loadSession(load: typeof getAuthStatus): Promise<AuthCheckResult> {
    const checkedRevision = ++revision
    return (async (): Promise<AuthCheckResult> => {
      try {
        const status = await load({ silent: true })
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
    })()
  }

  function beginTransition<T>(action: (expectedRevision: number) => Promise<T>): Promise<T> {
    invalidatePendingRequests()
    // Finish older cookie-changing calls before login/logout can set a new cookie.
    const prior = Promise.allSettled([transition, refreshBarrier])
    invalidatePendingCheck()
    const expectedRevision = revision
    loading.value = true
    const pending = prior.then(() => action(expectedRevision)).finally(() => {
      if (revision === expectedRevision)
        loading.value = false
    })
    transition = pending.then(() => {}, () => {})
    return pending
  }

  function login(payload: Parameters<typeof apiLogin>[0]) {
    return beginTransition(async (expectedRevision) => {
      try {
        await apiLogin(payload)
        if (revision !== expectedRevision)
          return false
        isAuthenticated.value = true
        sessionChecked.value = true
        resetUnauthorizedHandling()
        return true
      }
      catch {
        if (revision === expectedRevision)
          isAuthenticated.value = false
        return false
      }
    })
  }

  function logout() {
    return beginTransition(async (expectedRevision) => {
      try {
        await apiLogout({ silent: true })
      }
      catch {
        // Preserve local logout even when the server cannot confirm revocation.
      }
      finally {
        if (revision === expectedRevision) {
          isAuthenticated.value = false
          sessionChecked.value = true
        }
      }
    })
  }

  function invalidateSession() {
    invalidatePendingRequests()
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
    refreshSession,
    login,
    logout,
    invalidateSession,
  }
})
