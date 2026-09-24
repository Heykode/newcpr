import type { Ref } from 'vue'
import type { AccountGroup, GroupMonitorItem } from '@/api'
import { useDocumentVisibility, useEventListener, useIntervalFn } from '@vueuse/core'
import { computed, onScopeDispose, ref, shallowRef, watch } from 'vue'
import { getGroupMonitor } from '@/api'
import { orderMonitorGroups, readPinnedGroups } from '../components/group-monitor-presentation'

export function useGroupMonitor(groups: Ref<AccountGroup[]>, pageSize: Ref<number>) {
  const page = ref(0)
  const pins = shallowRef<string[]>([])
  const viewer = ref('')
  const records = shallowRef(new Map<string, GroupMonitorItem>())
  const sampleTimes = shallowRef(new Map<string, string>())
  const error = ref(false)
  const loading = ref(false)
  const refreshing = ref(false)
  const now = ref(Date.now())
  const visibility = useDocumentVisibility()
  const ordered = computed(() => orderMonitorGroups(groups.value, pins.value))
  const displayedCount = computed(() => ordered.value.length)
  const totalPages = computed(() => Math.max(1, Math.ceil(ordered.value.length / pageSize.value)))
  const visible = computed(() => ordered.value.slice(page.value * pageSize.value, (page.value + 1) * pageSize.value))
  const signature = computed(() => visible.value.map(group => group.id).join(','))
  // An all-empty catalog still needs the authenticated scope to restore saved pins.
  const requestSignature = computed(() => signature.value || (!viewer.value ? groups.value[0]?.id ?? '' : ''))
  const storageKey = computed(() => `cpr.accounts.monitor.pins.${viewer.value}`)
  const stale = computed(() => error.value || refreshing.value || visible.value.some((group) => {
    const time = Date.parse(sampleTimes.value.get(group.id) ?? '')
    return !Number.isFinite(time) || now.value - time > 45_000
  }))
  let controller: AbortController | undefined
  let generation = 0
  let disposed = false

  function restorePins() {
    try {
      pins.value = readPinnedGroups(JSON.parse(localStorage.getItem(storageKey.value) ?? '[]'))
    }
    catch {
      pins.value = []
    }
  }

  function togglePin(id: string) {
    if (!viewer.value)
      return
    pins.value = pins.value.includes(id) ? pins.value.filter(value => value !== id) : readPinnedGroups([id, ...pins.value])
    page.value = 0
    try {
      localStorage.setItem(storageKey.value, JSON.stringify(pins.value))
    }
    catch {
      // Private browsing may disallow storage; keep the preference for this visit.
    }
  }

  async function refresh(refreshForecasts = false) {
    controller?.abort()
    const owner = ++generation
    const ids = requestSignature.value ? requestSignature.value.split(',') : []
    if (disposed || visibility.value === 'hidden' || !ids.length) {
      loading.value = false
      return
    }
    const current = new AbortController()
    controller = current
    loading.value = true
    try {
      const response = await getGroupMonitor(ids, { signal: current.signal, silent: true, refreshForecasts })
      if (disposed || owner !== generation || current.signal.aborted)
        return
      if (response.viewerScope !== viewer.value) {
        viewer.value = response.viewerScope
        records.value = new Map()
        sampleTimes.value = new Map()
        refreshing.value = false
        restorePins()
      }
      const pending = response.pendingGroupIds ?? []
      const returnedIds = [...response.items.map(item => item.id), ...pending]
      if (returnedIds.length !== ids.length || new Set(returnedIds).size !== ids.length || ids.some(id => !returnedIds.includes(id)))
        throw new Error('Incomplete group monitor response')
      const next = new Map(records.value)
      const times = new Map(sampleTimes.value)
      for (const item of response.items) {
        if (ids.includes(item.id)) {
          next.set(item.id, item)
          times.set(item.id, response.generatedAt)
        }
      }
      const existing = new Set(groups.value.map(group => group.id))
      records.value = new Map([...next].filter(([id]) => existing.has(id)))
      sampleTimes.value = new Map([...times].filter(([id]) => existing.has(id)))
      now.value = Date.now()
      refreshing.value = Boolean(response.refreshing || pending.length)
      error.value = false
    }
    catch {
      if (!disposed && owner === generation && !current.signal.aborted)
        error.value = true
    }
    finally {
      if (owner === generation)
        loading.value = false
    }
  }

  function refreshNow() {
    if (!loading.value)
      return refresh(true)
  }

  watch(totalPages, value => page.value = Math.min(page.value, value - 1))
  watch(pageSize, (value, previous) => page.value = Math.floor(page.value * previous / value))
  watch([requestSignature, visibility], () => {
    void refresh()
  }, { immediate: true })
  useIntervalFn(() => {
    now.value = Date.now()
    if (!loading.value && visibility.value !== 'hidden')
      void refresh()
  }, 10_000)
  useEventListener(window, 'storage', (event) => {
    if (viewer.value && (event.key === storageKey.value || event.key === null)) {
      restorePins()
      page.value = 0
    }
  })
  onScopeDispose(() => {
    disposed = true
    generation += 1
    controller?.abort()
  })
  return { page, pins, viewer, visible, displayedCount, totalPages, records, stale, loading, refreshing, error, now, togglePin, refresh, refreshNow }
}
