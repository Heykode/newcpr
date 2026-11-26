import type { PricingCatalog, PricingChange, PricingSyncPreview } from '@/api'
import { computed, onMounted, onScopeDispose, ref, shallowRef, watch } from 'vue'
import { getPricing, previewPricingSync, syncPricing, updatePricing } from '@/api'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { useRequestState } from '@/composables/useRequestState'
import { t } from '@/i18n'
import { pricingProviders, pricingRows } from './model'

export function usePricing() {
  const catalog = shallowRef<PricingCatalog>({
    defaults: {},
    supportedBands: {},
    overrides: {},
    synced: {},
    syncedAt: null,
    syncedAtDisplay: null,
  })
  const provider = ref('')
  const search = ref('')
  const source = ref('all')
  const page = ref(1)
  const pageSize = ref(20)
  const selected = ref<string[]>([])
  const loadState = useRequestState()
  const error = loadState.error
  const writeAction = useAsyncAction()
  const previewState = useRequestState()
  let disposed = false
  onScopeDispose(() => {
    disposed = true
  })
  const preview = shallowRef<PricingSyncPreview>()
  const syncOpen = shallowRef(false)
  const syncProvider = ref('')
  // 确认窗口展示查询时的价格对比，保存后的列表刷新不能改变退场中的预览。
  const syncCatalog = shallowRef(catalog.value)
  const providers = computed(() => pricingProviders(catalog.value))
  const rows = computed(() => pricingRows(catalog.value, provider.value))
  const filtered = computed(() => rows.value.filter(row => row.model.toLowerCase().includes(search.value.trim().toLowerCase())
    && (source.value === 'all' || row.source === source.value)))
  const visible = computed(() => filtered.value.slice((page.value - 1) * pageSize.value, page.value * pageSize.value))
  const pagination = computed(() => ({ currentPage: page.value, pageSize: pageSize.value, total: filtered.value.length }))
  watch([provider, search, source, pageSize], () => {
    page.value = 1
  })
  // 筛选变更清空选择，避免批量修改屏幕之外的隐藏目标。
  watch([provider, search, source], () => {
    selected.value = []
  })
  watch(provider, () => previewState.invalidate(), { flush: 'sync' })
  watch(syncOpen, (open) => {
    if (!open)
      previewState.invalidate()
  })

  async function load() {
    if (disposed)
      return
    const id = loadState.start()
    try {
      const value = await getPricing({ signal: loadState.signal })
      if (!loadState.isCurrent(id))
        return
      catalog.value = value
      if (!providers.value.includes(provider.value))
        provider.value = providers.value[0] ?? ''
      error.value = ''
      page.value = Math.min(page.value, Math.max(1, Math.ceil(filtered.value.length / pageSize.value)))
    }
    catch (cause) {
      loadState.fail(id, cause)
    }
    finally { loadState.finish(id) }
  }
  async function save(models: string[], change: PricingChange): Promise<boolean> {
    if (disposed || writeAction.loading.value)
      return false
    const request = { provider: provider.value, models: [...models], change: JSON.parse(JSON.stringify(change)) as PricingChange }
    loadState.invalidate()
    return await writeAction.run(async () => {
      await updatePricing(request)
      if (disposed)
        return false
      toast.success(change.action === 'delete'
        ? t('settings.pricing.deleted')
        : change.action === 'reset'
          ? t('settings.pricing.manualOverrideCleared')
          : t('settings.pricing.modelPricingSaved'))
      selected.value = []
      await load()
      return true
    }) ?? false
  }
  async function startSync() {
    if (disposed || writeAction.loading.value)
      return
    const requestedProvider = provider.value
    const requestedCatalog = catalog.value
    const id = previewState.start()
    try {
      const value = await previewPricingSync({ signal: previewState.signal })
      if (!previewState.isCurrent(id))
        return
      preview.value = value
      syncProvider.value = requestedProvider
      syncCatalog.value = requestedCatalog
      syncOpen.value = true
    }
    catch (cause) { previewState.fail(id, cause) }
    finally { previewState.finish(id) }
  }
  async function confirmSync(models: Record<string, string[]>) {
    if (disposed || writeAction.loading.value || !syncOpen.value || !preview.value || !Object.values(models).some(items => items.length))
      return
    const approved = JSON.parse(JSON.stringify({ preview: preview.value, models })) as { preview: PricingSyncPreview, models: Record<string, string[]> }
    loadState.invalidate()
    await writeAction.run(async () => {
      await syncPricing(approved)
      if (disposed)
        return
      syncOpen.value = false
      toast.success(t('settings.pricing.synced'))
      await load()
    })
  }
  function toggle(model: string, checked: boolean) {
    selected.value = checked
      ? [
          ...new Set([...selected.value, model]),
        ]
      : selected.value.filter(id => id !== model)
  }
  function togglePage(checked: boolean) {
    const ids = visible.value.map(row => row.model)
    selected.value = checked
      ? [
          ...new Set([
            ...selected.value,
            ...ids,
          ]),
        ]
      : selected.value.filter(id => !ids.includes(id))
  }
  onMounted(load)
  return {
    catalog,
    providers,
    provider,
    search,
    source,
    page,
    pageSize,
    selected,
    error,
    rows,
    filtered,
    visible,
    pagination,
    loading: loadState.loading,
    saving: writeAction.loading,
    syncing: previewState.loading,
    syncError: previewState.error,
    preview,
    syncOpen,
    syncProvider,
    syncCatalog,
    save,
    startSync,
    confirmSync,
    toggle,
    togglePage,
  }
}
