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
  (response: AxiosResponse<unknown>) => {
    const error = normalizeApiResponseError(response)
    if (error)
      return rejectRequest(error, response.config)
    return response
  },
  (error: AxiosError<unknown>) => {
    return rejectRequest(normalizeApiError(error), error.config)
  },
)

async function rejectRequest(error: ApiError, config?: RequestConfig): Promise<never | AxiosResponse<unknown>> {
  if (error.kind === 'cancelled' || config?.signal?.aborted)
    return Promise.reject(error)

  if (config?.sessionGeneration !== undefined && config.sessionGeneration !== generation)
    return Promise.reject(error)
  const sessionExpired = error.status === 401 && error.code === 40101 && !isAuthenticationRequest(config?.url)
  if (sessionExpired && config && !config.authRetried && sessionRecoveryHandler) {
    try {
      if (!recovery && config.recoveryCount === successfulRecoveries) {
        const expectedGeneration = generation
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
      const authenticated = await (recovery ?? (config.recoveryCount !== successfulRecoveries))
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
