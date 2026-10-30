<script setup lang="ts">
import type { OutboundProxyRecord } from '@/api'
import { LockKeyhole, MoreHorizontal, Pencil, Plus, Power, RefreshCw, Search, Trash2, Users, Wifi, X } from '@lucide/vue'
import { watchDebounced } from '@vueuse/core'
import { computed, onMounted, reactive, ref, shallowRef, watch } from 'vue'
import { createProxy, deleteProxy, getProxies, probeProxy, testProxy, updateProxy } from '@/api'
import { defaultRequestLocation, testProxyBatch } from '@/api/modules/proxies'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseMenuItem from '@/components/base/BaseMenuItem.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BasePopover from '@/components/base/BasePopover.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import { defineTableColumns } from '@/components/base/BaseTable/columns'
import BaseTable from '@/components/base/BaseTable/index.vue'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { usePagedQuery } from '@/composables/usePagedQuery'
import { t } from '@/i18n'
import { formatDateTime } from '@/utils/date'
import ManagedProxyPanel from './components/ManagedProxyPanel.vue'
import ProxyAccountsModal from './components/ProxyAccountsModal.vue'
import ProxyBatchCreateModal from './components/ProxyBatchCreateModal.vue'
import ProxyFormModal from './components/ProxyFormModal.vue'
import { useProxyBatchTesting } from './composables/useProxyBatchTesting'
import { proxyLocationMessage, proxyTestFeedback } from './utils/location'

const search = shallowRef('')
const activeTab = shallowRef('regular')
const tabs = [{ id: 'regular', label: '普通代理' }, { id: 'subscriptions', label: '订阅管理' }, { id: 'dynamic', label: '动态代理' }, { id: 'nodes', label: '节点管理' }, { id: 'kernel', label: '内核与规则' }]
const query = usePagedQuery({
  initialPageSize: 20,
  load: (pagination, options) => getProxies({ ...pagination, search: search.value.trim() || undefined }, options),
})
const { items: proxies, loading } = query
const pagination = computed(() => ({ currentPage: query.page.value, pageSize: query.pageSize.value, total: query.total.value }))
const columns = defineTableColumns<OutboundProxyRecord>([
  { key: 'selection', label: '', kind: 'selection' },
  { key: 'identity', label: '代理', kind: 'identity' },
  { key: 'status', label: '状态', kind: 'status' },
  { key: 'exitIp', label: '出口 IP', kind: 'custom', size: 'xl' },
  { key: 'location', label: '地区 / 时区', kind: 'custom' },
  { key: 'latency', label: '耗时', kind: 'custom', size: 'sm' },
  { key: 'accounts', label: '关联账号', kind: 'custom', size: 'sm' },
  { key: 'testedAt', label: '测试时间', kind: 'datetime' },
  { key: 'actions', label: '操作', kind: 'actions' },
])
const showForm = shallowRef(false)
const editing = shallowRef<OutboundProxyRecord | null>(null)
const form = reactive({ name: '', proxyUrl: '', maxWebsocketConnections: 0, autoLocation: false, locationEnabled: false, location: defaultRequestLocation() })
const saveAction = useAsyncAction()
const { loading: saving } = saveAction
const deleteAction = useAsyncAction()
const { loading: deleting } = deleteAction
const showDelete = shallowRef(false)
const pendingDelete = shallowRef<OutboundProxyRecord | null>(null)
const testingIds = ref(new Set<string>())
const updatingIds = ref(new Set<string>())
const showDisable = shallowRef(false)
const pendingDisable = shallowRef<OutboundProxyRecord | null>(null)
const disabling = computed(() => pendingDisable.value !== null && updatingIds.value.has(pendingDisable.value.id))
const formTestAction = useAsyncAction()
const testingForm = computed(() => formTestAction.loading.value || (editing.value !== null && testingIds.value.has(editing.value.id)))
const showAccounts = shallowRef(false)
const inspected = shallowRef<OutboundProxyRecord | null>(null)
const showBatchCreate = ref(false)
const selected = ref(new Map<string, OutboundProxyRecord>())
const batch = useProxyBatchTesting({
  test: (proxies, options, detectLocation) => testProxyBatch(proxies.map(({ id, revision }) => ({ id, revision, detectLocation })), options),
  reload: () => query.execute({ silent: true }),
})
const batchTesting = batch.busy
const batchResults = computed(() => batch.tests.items.value.map(item => `${item.name}：${item.message || item.status}`))
const allSelected = computed(() => proxies.value.length > 0 && proxies.value.every(proxy => selected.value.has(proxy.id)))
watch(proxies, (items) => {
  for (const item of items) {
    if (selected.value.has(item.id))
      selected.value.set(item.id, item)
  }
})
function select(proxy: OutboundProxyRecord, value: boolean) {
  if (value)
    selected.value.set(proxy.id, proxy)
  else
    selected.value.delete(proxy.id)
}
function selectPage(value: boolean) {
  for (const proxy of proxies.value)
    select(proxy, value)
}
async function testSelected() {
  if (batchTesting.value || !selected.value.size)
    return
  await batch.start([...selected.value.keys()])
}

function openForm(proxy: OutboundProxyRecord | null = null) {
  editing.value = proxy
  form.name = proxy?.name ?? ''
  form.proxyUrl = ''
  form.autoLocation = proxy?.autoLocation ?? false
  form.maxWebsocketConnections = proxy?.maxWebsocketConnections ?? 0
  form.locationEnabled = Boolean(proxy?.requestLocation)
  form.location = proxy?.requestLocation ? { ...proxy.requestLocation } : defaultRequestLocation()
  showForm.value = true
}

async function checkProxy(proxy: OutboundProxyRecord, detectLocation = true) {
  if (batchTesting.value || testingIds.value.has(proxy.id) || updatingIds.value.has(proxy.id))
    return
  testingIds.value.add(proxy.id)
  try {
    const result = await testProxy({ id: proxy.id, revision: proxy.revision, detectLocation })
    if (editing.value?.id === result.id && editing.value.revision === proxy.revision)
      editing.value = result
    if (result.lastTest) {
      const feedback = proxyTestFeedback(result.lastTest)
      toast[feedback.tone](feedback.tone === 'warning' ? feedback.message : `${result.name}：${feedback.message}`)
    }
    else {
      toast.error('代理测试未返回结果')
    }
  }
  catch {}
  finally {
    testingIds.value.delete(proxy.id)
    await query.execute({ silent: true })
  }
}

async function testConnection() {
  if (saving.value || testingForm.value)
    return
  const proxyUrl = form.proxyUrl.trim()
  if (!proxyUrl && editing.value) {
    await formTestAction.run(() => checkProxy(editing.value!, false))
    return
  }
  if (!proxyUrl) {
    toast.warning('请填写代理连接地址')
    return
  }
  await formTestAction.run(async () => {
    const result = await probeProxy({ proxyUrl, detectLocation: false })
    const feedback = proxyTestFeedback(result)
    toast[feedback.tone](feedback.message)
  })
}

async function detectLocation() {
  if (saving.value || testingForm.value)
    return
  const proxyUrl = form.proxyUrl.trim()
  const proxy = editing.value
  if (!proxyUrl && !proxy) {
    toast.warning('请填写代理连接地址')
    return
  }
  await formTestAction.run(async () => {
    let result
    if (proxyUrl) {
      result = await probeProxy({ proxyUrl, detectLocation: true })
    }
    else {
      const updated = await testProxy({ id: proxy!.id, revision: proxy!.revision, detectLocation: true })
      if (editing.value?.id === updated.id && editing.value.revision === proxy!.revision)
        editing.value = updated
      result = updated.lastTest
      await query.execute({ silent: true })
    }
    if (result?.success && result.location?.status === 'detected') {
      form.location = { ...result.location.location }
      form.locationEnabled = true
      toast.success('已填入出口地区')
    }
    else if (result) {
      const feedback = proxyTestFeedback(result)
      toast[feedback.tone === 'success' ? 'warning' : feedback.tone](feedback.tone === 'success' ? '未获得出口地区' : feedback.message)
    }
  })
}

async function save() {
  if (saving.value || testingForm.value)
    return
  const name = form.name.trim()
  const proxyUrl = form.proxyUrl.trim()
  if (!Number.isInteger(form.maxWebsocketConnections) || form.maxWebsocketConnections < 0 || form.maxWebsocketConnections > 4_294_967_295) {
    toast.warning('WS 连接上限须为非负整数')
    return
  }
  if (!name || (!editing.value && !proxyUrl)) {
    toast.warning('请填写代理名称和连接地址')
    return
  }
  await saveAction.run(async () => {
    const requestLocation = form.locationEnabled ? { ...form.location } : null
    // 编辑时留空保留已保存的地址和认证，不能用脱敏地址覆盖原连接。
    const result = await (editing.value
      ? updateProxy({ id: editing.value.id, revision: editing.value.revision, name, proxyUrl: proxyUrl || undefined, maxWebsocketConnections: form.maxWebsocketConnections, autoLocation: form.autoLocation, requestLocation })
      : createProxy({ name, proxyUrl, maxWebsocketConnections: form.maxWebsocketConnections, autoLocation: form.autoLocation, requestLocation }))
    showForm.value = false
    toast.success('代理已保存')
    if (result.record.autoLocation && result.record.lastTest) {
      const feedback = proxyTestFeedback(result.record.lastTest)
      if (feedback.tone !== 'success')
        toast[feedback.tone](feedback.message)
    }
    search.value = ''
    await query.reloadFromStart()
  })
}

function requestDelete(proxy: OutboundProxyRecord) {
  pendingDelete.value = proxy
  showDelete.value = true
}

async function setEnabled(proxy: OutboundProxyRecord, enabled: boolean) {
  if (updatingIds.value.has(proxy.id) || testingIds.value.has(proxy.id))
    return
  updatingIds.value.add(proxy.id)
  try {
    await updateProxy({ id: proxy.id, revision: proxy.revision, name: proxy.name, enabled })
    showDisable.value = false
    toast.success(enabled ? '代理已启用' : '代理已禁用')
  }
  catch {}
  finally {
    updatingIds.value.delete(proxy.id)
    await query.execute({ silent: true })
  }
}

function toggleEnabled(proxy: OutboundProxyRecord) {
  if (proxy.enabled) {
    pendingDisable.value = proxy
    showDisable.value = true
  }
  else {
    void setEnabled(proxy, true)
  }
}

async function confirmDisable() {
  if (pendingDisable.value)
    await setEnabled(pendingDisable.value, false)
}

async function confirmDelete() {
  const proxy = pendingDelete.value
  if (!proxy || deleting.value)
    return
  await deleteAction.run(async () => {
    await deleteProxy({ id: proxy.id, revision: proxy.revision })
    selected.value.delete(proxy.id)
    showDelete.value = false
    await query.execute()
    toast.success('代理已删除')
  })
}

function setPage(page: number) {
  void query.execute({ page })
}

function setPageSize(size: number) {
  void query.execute({ page: 1, pageSize: size })
}

function clearForm() {
  if (!showForm.value) {
    form.proxyUrl = ''
  }
}
watchDebounced(search, () => query.reloadFromStart(), { debounce: 300 })
onMounted(() => void query.execute())
</script>

<template>
  <div class="flex h-full min-h-0 w-full flex-col overflow-hidden">
    <BasePageHeader
      class="h-17"
      :title="t('navigation.proxies')"
      :description="t('proxies.page.description')"
    />
    <nav class="flex shrink-0 gap-5 overflow-x-auto border-b border-cp-border" aria-label="代理管理分类">
      <button v-for="tab in tabs" :key="tab.id" class="whitespace-nowrap border-b-2 px-1 py-3 text-cp-sm" :class="activeTab === tab.id ? 'border-cp-primary font-medium text-cp-primary' : 'border-transparent text-cp-text-secondary'" :aria-current="activeTab === tab.id ? 'page' : undefined" @click="activeTab = tab.id">
        {{ tab.label }}
      </button>
    </nav>
    <ManagedProxyPanel v-if="activeTab !== 'regular'" :key="activeTab" :tab="activeTab" />
    <BaseCard v-else class="mt-5 flex min-h-0 flex-1 flex-col">
      <template #header>
        <div class="flex w-full flex-col gap-3 sm:flex-row sm:items-center">
          <BaseInput v-model="search" class="sm:w-80" :aria-label="t('proxies.page.searchLabel')" :placeholder="t('proxies.page.searchPlaceholder')">
            <template #prefix>
              <Search class="size-4.5 text-cp-text-tertiary" />
            </template>
          </BaseInput>
          <div class="flex shrink-0 flex-wrap items-center justify-end gap-2 sm:ml-auto">
            <BaseButton :disabled="!selected.size || testingIds.size > 0 || updatingIds.size > 0" :loading="batchTesting" @click="testSelected">
              <template #icon>
                <Wifi class="size-4" />
              </template>测试所选 ({{ selected.size }})
            </BaseButton>
            <BaseButton v-if="batch.tests.running.value" :disabled="batch.tests.stopping.value" @click="batch.tests.stop">
              停止后续测试
            </BaseButton>
            <BaseIconButton v-if="selected.size" label="清除代理选择" :disabled="batchTesting" @click="selected.clear()">
              <X class="size-4" />
            </BaseIconButton>
            <BaseButton :disabled="batchTesting" @click="showBatchCreate = true">
              <template #icon>
                <Plus class="size-4" />
              </template>批量新增
            </BaseButton>
            <BaseIconButton label="刷新代理列表" :loading="loading" @click="query.execute()">
              <RefreshCw class="size-4" />
            </BaseIconButton>
            <BaseButton variant="primary" @click="openForm()">
              <template #icon>
                <Plus class="size-4" />
              </template>
              {{ t('proxies.editor.createTitle') }}
            </BaseButton>
          </div>
        </div>
      </template>
      <template #body>
        <div class="flex h-full min-h-0 flex-col">
          <div v-if="batchResults.length" role="status" class="max-h-36 shrink-0 overflow-auto text-cp-xs">
            <p v-for="(result, index) in batchResults" :key="index">
              {{ result }}
            </p>
          </div>
          <BaseTable class="min-h-0 flex-1" :columns="columns" :rows="proxies" :loading="loading" :empty-text="search.trim() ? '没有找到匹配的代理，请尝试其他名称' : '暂无代理，请点击新增代理添加'">
            <template #header-selection>
              <BaseCheckbox :model-value="allSelected" :disabled="batchTesting" :label="t('proxies.page.selectCurrentPageProxy')" @update:model-value="selectPage" />
            </template>
            <template #selection="{ row }">
              <BaseCheckbox :model-value="selected.has(row.id)" :disabled="batchTesting" :label="`选择 ${row.name}`" @update:model-value="select(row, $event)" />
            </template>
            <template #identity="{ row }">
              <div class="grid min-w-0 gap-1">
                <strong class="truncate text-cp text-cp-text" :title="row.name">{{ row.name }}</strong>
                <span class="flex min-w-0 items-center gap-1 text-cp-xs font-emphasis text-cp-text-quaternary">
                  <LockKeyhole v-if="row.hasAuthentication" class="size-3 shrink-0" aria-label="已保存代理认证" />
                  <span class="truncate font-mono" :title="row.endpoint">{{ row.endpoint }}</span>
                </span>
              </div>
            </template>
            <template #exitIp="{ row }">
              <div v-if="row.lastTest?.exitIpv4 && row.lastTest?.exitIpv6" class="flex min-w-0 flex-col gap-0.5 font-mono text-cp-xs">
                <span class="truncate" :title="`IPv4: ${row.lastTest.exitIpv4}`">{{ row.lastTest.exitIpv4 }}</span>
                <span class="truncate" :title="`IPv6: ${row.lastTest.exitIpv6}`">{{ row.lastTest.exitIpv6 }}</span>
              </div>
              <div v-else-if="row.lastTest?.exitIpv4" class="block truncate font-mono text-cp-xs" :title="`IPv4: ${row.lastTest.exitIpv4}`">
                {{ row.lastTest.exitIpv4 }}
              </div>
              <div v-else-if="row.lastTest?.exitIpv6" class="block truncate font-mono text-cp-xs" :title="`IPv6: ${row.lastTest.exitIpv6}`">
                {{ row.lastTest.exitIpv6 }}
              </div>
              <span v-else-if="row.lastTest?.exitIp" class="block truncate font-mono text-cp-xs" :title="row.lastTest.exitIp">{{ row.lastTest.exitIp }}</span>
              <span v-else class="text-cp-text-quaternary">-</span>
            </template>
            <template #latency="{ row }">
              <span v-if="testingIds.has(row.id)" class="text-cp-text-secondary">{{ t('proxies.page.underTest') }}</span>
              <span v-else-if="row.lastTest?.success" class="tabular-nums text-cp-success">
                {{ row.lastTest.latencyMs }} ms
              </span>
              <span v-else-if="row.lastTest" class="text-cp-error" :title="`${row.lastTest.message}（耗时 ${row.lastTest.latencyMs} ms）`">{{ t('common.status.failed') }}</span>
              <span v-else class="text-cp-text-quaternary">{{ t('proxies.page.notTested') }}</span>
            </template>
            <template #location="{ row }">
              <div class="grid min-w-0 gap-1">
                <span class="text-cp-xs text-cp-text-quaternary">{{ row.autoLocation ? '自动' : row.requestLocation ? '自定义' : '默认' }}</span>
                <span class="truncate text-cp-xs text-cp-text-secondary" :title="proxyLocationMessage(row)">{{ proxyLocationMessage(row) }}</span>
              </div>
            </template>
            <template #accounts="{ row }">
              <button type="button" class="inline-flex cursor-pointer items-center gap-1.5 rounded-sm border-0 bg-transparent p-0 text-cp-sm text-cp-text-secondary outline-none transition-colors hover:text-cp-primary-text focus-visible:ring-2 focus-visible:ring-cp-control-outline focus-visible:ring-offset-2 focus-visible:ring-offset-cp-bg-container" :aria-label="`查看 ${row.name} 的 ${row.accountCount} 个关联账号`" @click="inspected = row; showAccounts = true">
                <Users class="size-3.5" aria-hidden="true" />
                <span class="font-mono tabular-nums">{{ row.accountCount }}</span>
              </button>
            </template>
            <template #testedAt="{ row }">
              {{ row.lastTestAt ? formatDateTime(row.lastTestAt) : '-' }}
            </template>
            <template #status="{ row }">
              <span :class="row.enabled ? 'text-cp-success' : 'text-cp-text-quaternary'">{{ row.enabled ? '已启用' : '已禁用' }}</span>
            </template>
            <template #actions="{ row }">
              <div class="flex items-center gap-0.5">
                <BaseIconButton size="sm" :label="t('proxies.actions.test')" :loading="testingIds.has(row.id)" :disabled="batchTesting || testingIds.has(row.id) || updatingIds.has(row.id)" @click="checkProxy(row)">
                  <Wifi class="size-3.5 text-cp-link" />
                </BaseIconButton>
                <BaseIconButton size="sm" :label="t('proxies.actions.edit')" :disabled="batchTesting || testingIds.has(row.id) || updatingIds.has(row.id)" @click="openForm(row)">
                  <Pencil class="size-3.5 text-cp-link" />
                </BaseIconButton>
                <BasePopover placement="bottom-end">
                  <template #trigger="{ open }">
                    <BaseIconButton size="sm" label="更多代理操作" :aria-expanded="open" :loading="updatingIds.has(row.id)" :disabled="batchTesting || testingIds.has(row.id) || updatingIds.has(row.id)">
                      <MoreHorizontal class="size-3.5" />
                    </BaseIconButton>
                  </template>
                  <template #default="{ close }">
                    <div class="w-44 p-1">
                      <BaseMenuItem :disabled="batchTesting || testingIds.has(row.id) || updatingIds.has(row.id)" @click="(close(), toggleEnabled(row))">
                        <template #icon>
                          <Power class="size-3.5" :class="row.enabled ? 'text-cp-warning' : 'text-cp-success'" />
                        </template>
                        {{ row.enabled ? '禁用代理' : '启用代理' }}
                      </BaseMenuItem>
                      <BaseMenuItem tone="destructive" :disabled="batchTesting || testingIds.has(row.id) || updatingIds.has(row.id)" @click="(close(), requestDelete(row))">
                        <template #icon>
                          <Trash2 class="size-3.5" />
                        </template>
                        {{ t('proxies.actions.deleteProxy') }}
                      </BaseMenuItem>
                    </div>
                  </template>
                </BasePopover>
              </div>
            </template>
          </BaseTable>
          <BaseTablePagination :pagination="pagination" :loading="loading" @page-change="setPage" @page-size-change="setPageSize" />
        </div>
      </template>
    </BaseCard>

    <ProxyFormModal
      v-model="showForm"
      v-model:name="form.name"
      v-model:proxy-url="form.proxyUrl"
      v-model:max-websocket-connections="form.maxWebsocketConnections"
      v-model:auto-location="form.autoLocation"
      v-model:location-enabled="form.locationEnabled"
      v-model:location="form.location"
      :proxy="editing"
      :saving="saving"
      :testing="testingForm"
      @after-leave="clearForm"
      @save="save"
      @test="testConnection"
      @detect-location="detectLocation"
    />
    <ProxyBatchCreateModal v-if="showBatchCreate" v-model="showBatchCreate" @created="query.execute({ silent: true })" />
    <BaseConfirmModal v-model="showDelete" :title="t('proxies.actions.deleteProxy')" destructive :loading="deleting" @confirm="confirmDelete">
      <p class="m-0">
        确定删除“{{ pendingDelete?.name }}”吗？
        将自动解除 {{ pendingDelete?.accountCount ?? 0 }} 个账号的代理绑定，原来使用此代理的请求将改为直连。
      </p>
    </BaseConfirmModal>
    <BaseConfirmModal v-model="showDisable" :title="t('proxies.actions.disableProxy')" :loading="disabling" @confirm="confirmDisable">
      <p class="m-0">
        禁用“{{ pendingDisable?.name }}”后保留配置和 {{ pendingDisable?.accountCount ?? 0 }} 个账号的绑定，原来使用此代理的后续请求将改为直连。重新启用后恢复使用。
      </p>
    </BaseConfirmModal>
    <ProxyAccountsModal v-model="showAccounts" :proxy="inspected" @removed="query.execute({ silent: true })" />
  </div>
</template>
