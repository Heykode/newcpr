import type {
  AxiosError,
  AxiosInstance,
  AxiosRequestConfig,
  AxiosResponse,
} from 'axios'
import type { ApiError } from './error'

import axios from 'axios'
import { toast } from '@/components/base/BaseToast'
import { API_BASE_URL, API_TIMEOUT_MS } from './constants'
import { normalizeApiError, normalizeApiResponseError } from './error'

export { ApiError } from './error'

export interface RequestOptions {
  // 静默只关闭全局提示，不吞掉异常，也不跳过会话失效处理。
  silent?: boolean
  signal?: AbortSignal
  timeout?: number
}

type RequestConfig = AxiosRequestConfig & Omit<RequestOptions, 'signal'> & {
  sessionGeneration?: number
  recoveryCount?: number
  authRetried?: boolean
  transientRetried?: boolean
}

const http: AxiosInstance = axios.create({
  baseURL: API_BASE_URL,
  timeout: API_TIMEOUT_MS,
  withCredentials: true,
})

let unauthorizedHandled = false
let unauthorizedHandler: (() => void | Promise<void>) | undefined
let sessionRecoveryHandler: (() => Promise<boolean>) | undefined
let recovery: Promise<boolean> | undefined
let generation = 0
let successfulRecoveries = 0

export function setSessionRecoveryHandler(handler: () => Promise<boolean>) {
  sessionRecoveryHandler = handler
}

export function setUnauthorizedHandler(handler: () => void | Promise<void>) {
  unauthorizedHandler = handler
}

export function resetUnauthorizedHandling() {
  unauthorizedHandled = false
}

export function invalidatePendingRequests() {
  generation += 1
  recovery = undefined
  successfulRecoveries = 0
  resetUnauthorizedHandling()
}

function isAuthenticationRequest(url?: string) {
  return ['/api/admin/auth/login', '/api/admin/auth/status', '/api/admin/auth/refresh', '/api/admin/auth/logout'].includes(url ?? '')
}

function handleUnauthorizedOnce() {
  if (unauthorizedHandled || !unauthorizedHandler)
    return
  unauthorizedHandled = true
  void Promise.resolve(unauthorizedHandler()).catch(() => {
    unauthorizedHandled = false
  })
}

http.interceptors.request.use(async (config) => {
  const tracked = config as typeof config & RequestConfig
  tracked.sessionGeneration ??= generation
  tracked.recoveryCount ??= successfulRecoveries
  if (recovery && !isAuthenticationRequest(config.url))
    await recovery
  if (tracked.sessionGeneration !== generation)
    throw new axios.CanceledError('Session changed')
  return config
})

http.interceptors.response.use(
  async (response: AxiosResponse<unknown>) => {
    await decodeBlobError(response)
    assertSessionGeneration((response.config as RequestConfig).sessionGeneration)
    const error = normalizeApiResponseError(response)
    if (error)
      return rejectRequest(error, response.config)
    return response
  },
  async (error: AxiosError<unknown>) => {
    if (error.response)
      await decodeBlobError(error.response)
    return rejectRequest(normalizeApiError(error), error.config)
  },
)

export function currentSessionGeneration() {
  return generation
}

export function assertSessionGeneration(expected?: number) {
  if (expected !== undefined && expected !== generation)
    throw new axios.CanceledError('Session changed')
}

async function recoverSession(expectedGeneration: number, recoveryCount: number) {
  assertSessionGeneration(expectedGeneration)
  if (!sessionRecoveryHandler)
    return false
  if (!recovery && recoveryCount === successfulRecoveries) {
    const pending = sessionRecoveryHandler().then((authenticated) => {
      if (generation !== expectedGeneration)
        return false
      if (authenticated)
        successfulRecoveries += 1
      return authenticated
    }).finally(() => {
      if (recovery === pending)
        recovery = undefined
    })
    recovery = pending
  }
  const authenticated = await (recovery ?? (recoveryCount !== successfulRecoveries))
  assertSessionGeneration(expectedGeneration)
  return authenticated
}

// A rejected authentication check is safe to replay; an accepted streaming POST is not.
export async function sessionFetch(url: string, init: RequestInit): Promise<Response> {
  const expected = generation
  const recoveryCount = successfulRecoveries
  if (recovery)
    await recovery
  assertSessionGeneration(expected)
  let response = await fetch(url, { ...init, credentials: 'include' })
  try {
    assertSessionGeneration(expected)
    if (response.status === 401) {
      const body: unknown = await response.clone().json().catch(() => null)
      if (body && typeof body === 'object' && 'code' in body && body.code === 40101) {
        if (await recoverSession(expected, recoveryCount)) {
          await response.body?.cancel()
          init.signal?.throwIfAborted()
          response = await fetch(url, { ...init, credentials: 'include' })
          assertSessionGeneration(expected)
        }
        if (response.status === 401)
          handleUnauthorizedOnce()
      }
    }
    return response
  }
  catch (error) {
    await response.body?.cancel().catch(() => {})
    throw error
  }
}

async function decodeBlobError(response: AxiosResponse<unknown>) {
  if (response.data instanceof Blob && response.data.size <= 64 * 1024
    && response.data.type.includes('json')) {
    try {
      response.data = JSON.parse(await response.data.text())
    }
    catch {
      // Keep malformed payloads for the normal HTTP error fallback.
    }
  }
}

async function rejectRequest(error: ApiError, config?: RequestConfig): Promise<never | AxiosResponse<unknown>> {
  if (error.kind === 'cancelled' || config?.signal?.aborted)
    return Promise.reject(error)

  if (config?.sessionGeneration !== undefined && config.sessionGeneration !== generation)
    return Promise.reject(error)
  const sessionExpired = error.status === 401 && error.code === 40101 && !isAuthenticationRequest(config?.url)
  if (sessionExpired && config && !config.authRetried && sessionRecoveryHandler) {
    try {
      const authenticated = await recoverSession(config.sessionGeneration ?? generation, config.recoveryCount ?? successfulRecoveries)
      if (authenticated && !config.signal?.aborted && config.sessionGeneration === generation)
        return http.request({ ...config, authRetried: true } as RequestConfig)
    }
    catch (cause) {
      // Temporary renewal failure does not prove the administrator logged out.
      return Promise.reject(cause)
    }
  }
  if (config?.sessionGeneration !== undefined && config.sessionGeneration !== generation)
    return Promise.reject(error)
  const method = config?.method?.toUpperCase() ?? 'GET'
  if (config && !config.transientRetried && ['GET', 'HEAD'].includes(method)
    && (error.status === 0 || [502, 503, 504].includes(error.status))) {
    await new Promise(resolve => setTimeout(resolve, 200))
    if (!config.signal?.aborted && config.sessionGeneration === generation)
      return http.request({ ...config, transientRetried: true } as RequestConfig)
  }
  const alreadyHandled = sessionExpired && unauthorizedHandled
  if (sessionExpired)
    handleUnauthorizedOnce()
  if (!config?.silent && !alreadyHandled)
    toast.error(error.message)
  return Promise.reject(error)
}

interface ApiEnvelope {
  code: number
  message: string
  data: unknown
}

function isApiEnvelope(value: unknown): value is ApiEnvelope {
  return (
    typeof value === 'object'
    && value !== null
    && 'data' in value
    && 'code' in value && typeof value.code === 'number'
    && 'message' in value && typeof value.message === 'string'
  )
}

export default async function request<T = unknown>(config: RequestConfig): Promise<T> {
  const response = await http.request<unknown>({
    ...config,
  })

  if (isApiEnvelope(response.data)) {
    return response.data.data as T
  }

  return response.data as T
}
