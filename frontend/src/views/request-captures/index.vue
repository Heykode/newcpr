<script setup lang="ts">
import type { CaptureConfig, CapturePage, CaptureStatus, CaptureTask } from '@/api/modules/request-capture'
import { Download, Eye, Play, RefreshCw, Save, Square, Trash2, X } from '@lucide/vue'
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { getAccountGroups } from '@/api/modules/account-groups'
import { getAccounts } from '@/api/modules/accounts'
import { getApiKeys } from '@/api/modules/api-keys'
import { captureExportUrl, configureRequestCaptures, createRequestCapture, deleteRequestCapture, getRequestCaptures, readRequestCapture, stopRequestCapture } from '@/api/modules/request-capture'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import FormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseNumberInput from '@/components/base/BaseNumberInput.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { formatDateTime } from '@/utils/date'

const status = ref<CaptureStatus | null>(null)
const config = ref<CaptureConfig | null>(null)
const scope = ref<CaptureTask['scope']>('account')
const targetId = ref('')
const targetSearch = ref('')
const targets = ref<{ value: string, label: string, description: string }[]>([])
const targetsLoading = ref(false)
const targetsFailed = ref(false)
const minutes = ref(15)
const includeMedia = ref(false)
const requestFilter = ref('')
const action = useAsyncAction()
const readAction = useAsyncAction()
const readId = ref<string | null>(null)
const page = ref<CapturePage | null>(null)
const readError = ref(false)
const loading = ref(false)
const failed = ref(false)
const deleteId = ref<string | null>(null)
const deleteOpen = ref(false)
const records = computed(() => status.value?.records.filter(record => record.requestId.includes(requestFilter.value.trim())) ?? [])
const labels = { running: '采集中', stopped: '已停止', expired: '已到期', interrupted: '重启中断' }
const scopes = [{ label: '账号', value: 'account' }, { label: 'API Key', value: 'key' }, { label: '分组', value: 'group' }]
let alive = true
let generation = 0
let timer: ReturnType<typeof setTimeout> | undefined
let listController: AbortController | undefined
let bodyController: AbortController | undefined
let targetController: AbortController | undefined
let searchTimer: ReturnType<typeof setTimeout> | undefined

async function searchTargets() {
  targetController?.abort()
  const controller = new AbortController()
  targetController = controller
  targetsLoading.value = true
  targetsFailed.value = false
  const search = targetSearch.value.trim() || undefined
  const options = { signal: controller.signal, silent: true }
  try {
    const selected = scope.value
    const result = selected === 'account'
      ? await getAccounts({ page: 1, pageSize: 30, search }, options)
      : selected === 'group'
        ? await getAccountGroups({ page: 1, pageSize: 30, search }, options)
        : await getApiKeys({ limit: 30, search }, options)
    if (alive && !controller.signal.aborted)
      targets.value = result.items.map(item => ({ value: item.id, label: item.name, description: item.id }))
  }
  catch {
    if (alive && !controller.signal.aborted)
      targetsFailed.value = true
  }
  finally {
    if (alive && !controller.signal.aborted)
      targetsLoading.value = false
  }
}
watch([scope, targetSearch], () => {
  targetController?.abort()
  clearTimeout(searchTimer)
  targetId.value = ''
  targets.value = []
  if (status.value?.config.enabled)
    searchTimer = setTimeout(() => void searchTargets(), 250)
})
watch(() => status.value?.config.enabled, (enabled) => {
  if (enabled)
    void searchTargets()
  else
    targetController?.abort()
})

async function load() {
  if (!alive || loading.value)
    return
  loading.value = true
  const epoch = generation
  listController = new AbortController()
  try {
    const next = await getRequestCaptures({ silent: true, signal: listController.signal })
    if (!alive || epoch !== generation)
      return
    status.value = next
    config.value ??= { ...next.config }
    failed.value = false
    if (readId.value && !next.records.some(record => record.id === readId.value))
      closeBody()
  }
  catch {
    if (alive && epoch === generation)
      failed.value = true
  }
  finally {
    loading.value = false
  }
}
async function poll() {
  await load()
  if (alive)
    timer = setTimeout(poll, 5000)
}
async function save() {
  if (!config.value)
    return
  const submitted = { ...config.value }
  await action.run(async () => {
    await configureRequestCaptures(submitted)
    generation++
    listController?.abort()
    if (!alive)
      return
    if (status.value)
      status.value.config = submitted
    if (!submitted.enabled)
      closeBody()
    await load()
  })
}
async function create() {
  const submitted = { scope: scope.value, targetId: targetId.value.trim(), minutes: minutes.value, includeMedia: includeMedia.value }
  await action.run(async () => {
    await createRequestCapture(submitted)
    await load()
  })
}
async function stop(id: string) {
  await action.run(async () => {
    await stopRequestCapture(id)
    await load()
  })
}
async function remove(id: string) {
  deleteId.value = id
  deleteOpen.value = true
}
async function confirmDelete() {
  const id = deleteId.value
  if (!id)
    return
  await action.run(async () => {
    await deleteRequestCapture(id)
    deleteOpen.value = false
    deleteId.value = null
    closeBody()
    await load()
  })
}
function closeBody() {
  bodyController?.abort()
  readId.value = null
  page.value = null
  readError.value = false
}
async function show(id: string, offset = 0) {
  if (readAction.loading.value)
    return
  bodyController?.abort()
  const controller = new AbortController()
  bodyController = controller
  readId.value = id
  page.value = null
  readError.value = false
  await readAction.run(async () => {
    try {
      const result = await readRequestCapture(id, offset, { signal: controller.signal, silent: true })
      if (alive && !controller.signal.aborted && readId.value === id)
        page.value = result
    }
    catch {
      if (alive && !controller.signal.aborted)
        readError.value = true
    }
  })
}
onMounted(() => {
  void poll()
})
onBeforeUnmount(() => {
  alive = false
  clearTimeout(timer)
  listController?.abort()
  targetController?.abort()
  clearTimeout(searchTimer)
  closeBody()
})
</script>

<template>
  <div class="flex min-w-0 flex-col gap-5">
    <BaseConfirmModal v-model="deleteOpen" title="删除采集任务" destructive :loading="action.loading.value" @confirm="confirmDelete">
      删除本任务及所有已采集正文？
    </BaseConfirmModal>
    <BasePageHeader title="请求采集">
      <template #actions>
        <BaseIconButton label="刷新" :disabled="loading" @click="load">
          <RefreshCw class="size-4" />
        </BaseIconButton>
      </template>
    </BasePageHeader>
    <p v-if="failed || status?.storageFault" role="alert" class="text-cp-error">
      {{ status?.storageFault ? '采集存储异常，已停采。' : '读取失败，请重试。' }}
    </p>
    <form v-if="config" class="flex flex-wrap items-end gap-4 border-y border-cp-border py-4" @submit.prevent="save">
      <BaseSwitch v-model="config.enabled" label="启用错误采集" show-label />
      <div class="grid gap-2 text-cp-sm">
        <span>磁盘配额</span><BaseNumberInput v-model="config.quotaMib" label="磁盘配额" unit="MiB" :min="1" :max="102400" />
      </div>
      <div class="grid gap-2 text-cp-sm">
        <span>保留时间</span><BaseNumberInput v-model="config.retentionDays" label="保留时间" unit="天" :min="1" :max="30" />
      </div>
      <BaseButton type="submit" :disabled="action.loading.value">
        <Save class="size-4" />保存
      </BaseButton>
    </form>
    <form v-if="status?.config.enabled" class="grid items-end gap-3 sm:grid-cols-2 xl:grid-cols-5" @submit.prevent="create">
      <FormItem label="范围">
        <BaseSelect v-model="scope" :options="scopes" />
      </FormItem>
      <FormItem label="目标">
        <div class="grid min-w-0 grid-cols-1 gap-2">
          <BaseInput v-model="targetSearch" aria-label="搜索采集目标" placeholder="搜索名称" />
          <BaseSelect id="capture-target-select" v-model="targetId" class="min-w-0 w-full" aria-label="选择采集目标" :options="targets" :disabled="targetsLoading" :empty-text="targetsFailed ? '读取失败，请重新搜索' : '暂无匹配结果'" />
        </div>
      </FormItem>
      <div class="grid gap-2 text-cp-sm">
        <span>采集时长</span><BaseNumberInput v-model="minutes" label="采集时长" unit="分钟" :min="1" :max="1440" />
      </div>
      <BaseSwitch v-model="includeMedia" label="保存媒体正文" show-label />
      <BaseButton type="submit" :disabled="!targetId.trim() || action.loading.value || status.storageFault">
        <Play class="size-4" />开始采集
      </BaseButton>
      <p class="text-cp-sm text-cp-text-secondary sm:col-span-2 xl:col-span-5">
        采集正文可能包含敏感对话、工具内容和用户文件。
      </p>
    </form>
    <div class="flex flex-wrap gap-5 text-cp-sm text-cp-text-secondary">
      <span>当前会话 {{ status?.activeSessions ?? 0 }}</span>
      <span>缓冲 {{ ((status?.bufferedBytes ?? 0) / 1024 / 1024).toFixed(1) }} MiB</span>
      <span>跳过 {{ status?.skipped ?? 0 }}</span>
    </div>
    <section class="min-w-0">
      <h2 class="mb-3 text-lg font-semibold">
        采集任务
      </h2>
      <div class="overflow-x-auto">
        <table class="w-full text-left text-cp-sm">
          <thead>
            <tr>
              <th class="p-3">
                目标
              </th><th class="p-3">
                状态
              </th><th class="p-3">
                到期时间
              </th><th class="p-3">
                操作
              </th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="task in status?.tasks ?? []" :key="task.id" class="border-t border-cp-border">
              <td class="max-w-72 truncate p-3 font-mono" :title="`${task.scope}: ${task.targetId}`">
                {{ task.scope }}: {{ task.targetId }}
              </td>
              <td class="whitespace-nowrap p-3">
                {{ labels[task.status] }}
              </td><td class="whitespace-nowrap p-3">
                {{ formatDateTime(task.expiresAt) }}
              </td>
              <td class="p-3">
                <div class="flex gap-1">
                  <BaseIconButton v-if="task.status === 'running'" label="停止采集" :disabled="action.loading.value" @click="stop(task.id)">
                    <Square class="size-4" />
                  </BaseIconButton>
                  <BaseIconButton label="删除任务" :disabled="action.loading.value" @click="remove(task.id)">
                    <Trash2 class="size-4" />
                  </BaseIconButton>
                  <a v-if="task.status !== 'running' && status?.config.enabled" :href="captureExportUrl('task', task.id)" download aria-label="导出任务" title="导出任务" class="p-2"><Download class="size-4" /></a>
                </div>
              </td>
            </tr>
            <tr v-if="!status?.tasks.length">
              <td colspan="4" class="p-6 text-center text-cp-text-secondary">
                暂无任务
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>
    <section class="min-w-0">
      <div class="mb-3 flex flex-wrap items-center justify-between gap-3">
        <h2 class="text-lg font-semibold">
          错误记录
        </h2>
        <BaseInput v-model="requestFilter" aria-label="筛选 Request ID" placeholder="Request ID" class="max-w-80" />
      </div>
      <div class="overflow-x-auto">
        <table class="w-full text-left text-cp-sm">
          <thead>
            <tr>
              <th class="p-3">
                Request ID
              </th><th class="p-3">
                内容
              </th><th class="p-3">
                大小
              </th><th class="p-3">
                时间
              </th><th class="p-3">
                操作
              </th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="record in records" :key="record.id" class="border-t border-cp-border">
              <td class="max-w-72 truncate p-3 font-mono" :title="record.requestId">
                {{ record.requestId }}
              </td>
              <td class="p-3">
                {{ record.incomplete ? '部分缺失' : '已收尾' }}
              </td><td class="whitespace-nowrap p-3">
                {{ (record.bytes / 1024).toFixed(1) }} KiB
              </td>
              <td class="whitespace-nowrap p-3">
                {{ formatDateTime(record.createdAt) }}
              </td>
              <td class="p-3">
                <div v-if="status?.config.enabled" class="flex items-center gap-2">
                  <BaseIconButton label="查看正文" :disabled="readAction.loading.value" @click="show(record.id)">
                    <Eye class="size-4" />
                  </BaseIconButton>
                  <a :href="captureExportUrl('record', record.id)" download aria-label="导出记录" title="导出记录" class="p-2"><Download class="size-4" /></a>
                </div>
              </td>
            </tr>
            <tr v-if="!records.length">
              <td colspan="5" class="p-6 text-center text-cp-text-secondary">
                暂无错误记录
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>
    <section v-if="readId" class="min-w-0 border-t border-cp-border pt-4">
      <div class="mb-3 flex items-center justify-between gap-3">
        <h2 class="text-lg font-semibold">
          采集正文
        </h2><BaseIconButton label="关闭正文" @click="closeBody">
          <X class="size-4" />
        </BaseIconButton>
      </div>
      <p v-if="readError" role="alert" class="text-cp-error">
        正文读取失败。
      </p>
      <pre class="max-h-96 overflow-auto whitespace-pre-wrap break-all text-xs">{{ readAction.loading.value ? '加载中...' : page?.text }}</pre>
      <BaseButton v-if="page?.nextOffset != null" :disabled="readAction.loading.value" @click="show(readId, page.nextOffset)">
        下一页
      </BaseButton>
    </section>
  </div>
</template>
