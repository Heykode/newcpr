import type { UsageFilterDraft } from '../utils/filters'
import { computed, shallowRef, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { readUsageFilterDraft, usageFilterError, usageFilterKeys, usageFilterParams } from '../utils/filters'

export function useUsageFilters() {
  const route = useRoute()
  const router = useRouter()
  const draft = shallowRef(readUsageFilterDraft(route.query))
  const error = computed(() => usageFilterError(draft.value))
  const validDraft = shallowRef<UsageFilterDraft>(error.value ? {} : draft.value)
  const written = new Set<string>()
  watch(draft, (value) => {
    if (usageFilterError(value))
      return
    const normalized = readUsageFilterDraft(value)
    validDraft.value = normalized
    const next = { ...route.query }
    for (const key of usageFilterKeys)
      delete next[key]
    for (const [key, item] of Object.entries(normalized))
      next[key] = String(item)
    if (usageFilterKeys.some(key => route.query[key] !== next[key])) {
      const key = JSON.stringify(readUsageFilterDraft(next))
      written.add(key)
      void router.replace({ query: next }).catch(() => {}).finally(() => written.delete(key))
    }
  }, { deep: true, immediate: true })
  watch(() => route.query, (query) => {
    const next = readUsageFilterDraft(query)
    if (written.delete(JSON.stringify(next)))
      return
    if (JSON.stringify(next) !== JSON.stringify(readUsageFilterDraft(draft.value)))
      draft.value = next
  })
  return {
    draft,
    error,
    filters: computed(() => usageFilterParams(validDraft.value)),
    errorFilters: computed(() => usageFilterParams(validDraft.value, true)),
  }
}
