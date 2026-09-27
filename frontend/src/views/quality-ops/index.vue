<script setup lang="ts">
import type { QualityRule, QualityRuleConfig, QualityRun } from '@/api/modules/quality-ops'
import { ClipboardCheck, Eye, Pause, Pencil, Play, Plus, RefreshCw, Save, Trash2 } from '@lucide/vue'
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { getAccountGroups } from '@/api/modules/account-groups'
import { getAccounts } from '@/api/modules/accounts'
import { deleteQualityRule, getQualityDetail, getQualityRules, getQualityRuns, runQualityRule, saveQualityRule } from '@/api/modules/quality-ops'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import FormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseNumberInput from '@/components/base/BaseNumberInput.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import BaseTextarea from '@/components/base/BaseTextarea.vue'
import { formatDateTime } from '@/utils/date'
import QualityDrawer from './QualityDrawer.vue'

const rules = ref<QualityRule[]>([])
const runs = ref<QualityRun[]>([])
const selectedId = ref('')
const selected = computed(() => rules.value.find(rule => rule.id === selectedId.value))
const filter = ref('')
const loading = ref(false)
const historyLoading = ref(false)
const busy = ref(false)
const error = ref('')
const historyError = ref('')
const editorError = ref('')
const editorOpen = ref(false)
const editing = ref<QualityRule | null>(null)
const deleteTarget = ref<QualityRule | null>(null)
const deleteOpen = ref(false)
const detailOpen = ref(false)
const detail = ref<QualityRun | null>(null)
const detailError = ref('')
const accountSearch = ref('')
const groupSearch = ref('')
const accounts = ref<{ value: string, label: string, description: string }[]>([])
const groups = ref<{ value: string, label: string }[]>([])
const names = ref<Record<string, string>>({})
const catalogError = ref('')
const efforts = ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'].map(value => ({ value, label: value }))
const labels: Record<string, string> = {
  correct: '通过',
  incorrect: '答案不符',
  unknown: '不确定',
  request_error: '请求错误',
  running: '检测中',
  interrupted: '执行中断',
  cancelled: '已取消',
}
const visibleRules = computed(() => rules.value.filter(rule => `${names.value[rule.config.accountId] ?? ''} ${rule.config.accountId} ${rule.config.model}`.toLowerCase().includes(filter.value.toLowerCase())))
const counts = computed(() => ({
  enabled: rules.value.filter(rule => rule.config.enabled).length,
  running: rules.value.filter(rule => rule.running).length,
  incorrect: rules.value.filter(rule => rule.lastStatus === 'incorrect').length,
  errors: rules.value.filter(rule => rule.lastStatus === 'request_error').length,
}))
function defaults(): QualityRuleConfig {
  return {
    accountId: '',
    model: '',
    enabled: true,
    cron: '0 */6 * * *',
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC',
    repetitions: 1,
    prompt: '',
    referenceAnswer: '',
    reasoningEffort: null,
    judgeGroupId: '',
    judgeModel: '',
    judgePrompt: '判断实际答案是否与参考答案一致。无法确定时返回 unknown。',
  }
}
const draft = ref(defaults())
const effort = computed({
  get: () => draft.value.reasoningEffort ?? '',
  set: (value: string) => {
    draft.value.reasoningEffort = value || null
  },
})
let alive = true
let timer: ReturnType<typeof setTimeout> | undefined
let searchTimer: ReturnType<typeof setTimeout> | undefined
let listController: AbortController | undefined
let historyController: AbortController | undefined
let detailController: AbortController | undefined
let catalogController: AbortController | undefined
let listVersion = 0

function message(cause: unknown) {
  return cause instanceof Error ? cause.message : '操作失败，请重试'
}
function statusColor(status: string | null) {
  return status === 'correct' ? 'text-cp-success' : status === 'incorrect' || status === 'request_error' ? 'text-cp-error' : 'text-cp-text-secondary'
}
function ruleStatusColor(rule: QualityRule) {
  if (rule.running || rule.pending)
    return 'text-cp-primary'
  return statusColor(rule.config.enabled ? rule.lastStatus : null)
}
function accountName(id: string) {
  return names.value[id] || id
}

async function load() {
  const version = ++listVersion
  listController?.abort()
  const controller = new AbortController()
  listController = controller
  loading.value = true
  try {
    const result = await getQualityRules({ signal: controller.signal, silent: true })
    if (!alive || controller.signal.aborted || version !== listVersion)
      return
    rules.value = result
    error.value = ''
    if (!result.some(rule => rule.id === selectedId.value))
      selectedId.value = result[0]?.id ?? ''
  }
  catch (cause) {
    if (alive && !controller.signal.aborted)
      error.value = message(cause)
  }
  finally {
    if (alive && version === listVersion)
      loading.value = false
  }
}
async function history() {
  historyController?.abort()
  const id = selectedId.value
  if (!id) {
    runs.value = []
    return
  }
  const controller = new AbortController()
  historyController = controller
  historyLoading.value = true
  try {
    const result = await getQualityRuns(id, { signal: controller.signal, silent: true })
    if (alive && !controller.signal.aborted && selectedId.value === id) {
      runs.value = result
      historyError.value = ''
    }
  }
  catch (cause) {
    if (alive && !controller.signal.aborted)
      historyError.value = message(cause)
  }
  finally {
    if (alive && !controller.signal.aborted)
      historyLoading.value = false
  }
}
watch(selectedId, () => {
  runs.value = []
  historyError.value = ''
  void history()
})
async function poll() {
  if (!document.hidden) {
    await load()
    await history()
  }
  if (alive)
    timer = setTimeout(poll, 10000)
}
async function refresh() {
  await load()
  await history()
}
async function catalogs() {
  catalogController?.abort()
  const controller = new AbortController()
  catalogController = controller
  try {
    const options = { signal: controller.signal, silent: true }
    const [accountPage, groupPage] = await Promise.all([
      getAccounts({ page: 1, pageSize: 50, search: accountSearch.value.trim() || undefined }, options),
      getAccountGroups({ page: 1, pageSize: 50, search: groupSearch.value.trim() || undefined }, options),
    ])
    if (!alive || controller.signal.aborted)
      return
    accounts.value = accountPage.items.map(account => ({ value: account.id, label: account.name, description: account.id }))
    for (const account of accountPage.items)
      names.value[account.id] = account.name
    groups.value = groupPage.items.filter(group => group.enabled).map(group => ({ value: group.id, label: group.name }))
    if (draft.value.accountId && !accounts.value.some(item => item.value === draft.value.accountId))
      accounts.value.unshift({ value: draft.value.accountId, label: accountName(draft.value.accountId), description: draft.value.accountId })
    if (draft.value.judgeGroupId && !groups.value.some(item => item.value === draft.value.judgeGroupId))
      groups.value.unshift({ value: draft.value.judgeGroupId, label: draft.value.judgeGroupId })
    catalogError.value = ''
  }
  catch (cause) {
    if (alive && !controller.signal.aborted)
      catalogError.value = message(cause)
  }
}
watch([accountSearch, groupSearch], () => {
  catalogController?.abort()
  clearTimeout(searchTimer)
  searchTimer = setTimeout(() => void catalogs(), 250)
})
function edit(rule: QualityRule | null) {
  editing.value = rule ? { ...rule, config: { ...rule.config } } : null
  draft.value = rule ? { ...rule.config } : defaults()
  editorError.value = ''
  editorOpen.value = true
  void catalogs()
}
async function save() {
  if (busy.value)
    return
  busy.value = true
  const data = { id: editing.value?.id ?? null, revision: editing.value?.revision ?? null, config: { ...draft.value } }
  try {
    const result = await saveQualityRule(data)
    if (!alive)
      return
    editorOpen.value = false
    selectedId.value = result.id
    await load()
    await history()
  }
  catch (cause) {
    if (alive)
      editorError.value = message(cause)
  }
  finally { busy.value = false }
}
async function mutate(action: () => Promise<unknown>) {
  if (busy.value)
    return
  busy.value = true
  try {
    await action()
    if (!alive)
      return
    deleteOpen.value = false
    deleteTarget.value = null
    await load()
    await history()
  }
  catch (cause) {
    if (alive)
      error.value = message(cause)
  }
  finally { busy.value = false }
}
function toggle(rule: QualityRule) {
  void mutate(() => saveQualityRule({ id: rule.id, revision: rule.revision, config: { ...rule.config, enabled: !rule.config.enabled } }))
}
function remove(rule: QualityRule) {
  deleteTarget.value = { ...rule, config: { ...rule.config } }
  deleteOpen.value = true
}
function confirmDelete() {
  const rule = deleteTarget.value
  if (rule)
    void mutate(() => deleteQualityRule({ id: rule.id, revision: rule.revision }))
}
async function show(run: QualityRun) {
  detailController?.abort()
  const controller = new AbortController()
  detailController = controller
  detail.value = null
  detailError.value = ''
  detailOpen.value = true
  try {
    const result = await getQualityDetail(run.id, { signal: controller.signal, silent: true })
    if (alive && !controller.signal.aborted)
      detail.value = result
  }
  catch (cause) {
    if (alive && !controller.signal.aborted)
      detailError.value = message(cause)
  }
}
watch(detailOpen, (open) => {
  if (!open) {
    detailController?.abort()
    detail.value = null
  }
})
onMounted(() => {
  void catalogs()
  void poll()
})
onBeforeUnmount(() => {
  alive = false
  clearTimeout(timer)
  clearTimeout(searchTimer)
  for (const controller of [listController, historyController, detailController, catalogController])
    controller?.abort()
})
</script>

<template>
  <div class="flex min-w-0 flex-col gap-5">
    <BasePageHeader title="质量运维" class="quality-header">
      <template #actions>
        <BaseIconButton label="刷新" :disabled="loading || historyLoading" @click="refresh">
          <RefreshCw class="size-4" />
        </BaseIconButton>
        <BaseButton variant="primary" :disabled="busy" @click="edit(null)">
          <Plus class="size-4" />新建规则
        </BaseButton>
      </template>
    </BasePageHeader>
    <p v-if="error" role="alert" class="text-cp-error">
      {{ error }}
    </p>
    <div class="grid grid-cols-2 border-y border-cp-border sm:grid-cols-4">
      <div class="border-b border-cp-border px-4 py-3 sm:border-b-0 sm:border-r">
        <div class="flex items-center gap-3">
          <span class="text-cp-sm text-cp-text-secondary">启用规则</span>
          <strong class="text-xl tabular-nums">{{ counts.enabled }}</strong>
        </div>
      </div>
      <div class="border-b border-cp-border px-4 py-3 sm:border-b-0 sm:border-r">
        <div class="flex items-center gap-3">
          <span class="text-cp-sm text-cp-text-secondary">检测中</span>
          <strong class="text-xl tabular-nums">{{ counts.running }}</strong>
        </div>
      </div>
      <div class="px-4 py-3 sm:border-r sm:border-cp-border">
        <div class="flex items-center gap-3">
          <span class="text-cp-sm text-cp-text-secondary">最近答案不符</span>
          <strong class="text-xl tabular-nums text-cp-warning">{{ counts.incorrect }}</strong>
        </div>
      </div>
      <div class="px-4 py-3">
        <div class="flex items-center gap-3">
          <span class="text-cp-sm text-cp-text-secondary">最近请求错误</span>
          <strong class="text-xl tabular-nums text-cp-error">{{ counts.errors }}</strong>
        </div>
      </div>
    </div>
    <div class="grid min-w-0 gap-5 xl:grid-cols-[minmax(260px,340px)_minmax(0,1fr)]">
      <section class="min-w-0">
        <div class="mb-3 flex items-center justify-between gap-3">
          <h2 class="text-base font-semibold">
            检测规则
          </h2>
          <span class="text-cp-sm tabular-nums text-cp-text-secondary">{{ rules.length }}</span>
        </div>
        <BaseInput v-model="filter" aria-label="筛选规则" placeholder="搜索账号或模型" />
        <div class="mt-3 max-h-[65vh] space-y-1 overflow-y-auto pr-1">
          <div v-for="rule in visibleRules" :key="rule.id" class="quality-rule rounded-lg border py-3 transition-colors" :class="selectedId === rule.id ? 'quality-rule-selected border-cp-border bg-cp-bg-container' : 'border-transparent hover:bg-cp-bg-container'">
            <button type="button" class="grid w-full min-w-0 gap-1 px-3 text-left text-cp-text outline-none focus-visible:ring-2 focus-visible:ring-cp-control-outline" :aria-pressed="selectedId === rule.id" @click="selectedId = rule.id">
              <span class="truncate font-semibold" :title="accountName(rule.config.accountId)">{{ accountName(rule.config.accountId) }}</span>
              <span class="truncate font-mono text-cp-sm text-cp-text-secondary" :title="rule.config.model">{{ rule.config.model }}</span>
              <span class="mt-1 flex items-center gap-1.5 text-xs" :class="ruleStatusColor(rule)"><span class="size-1.5 shrink-0 rounded-full bg-current" aria-hidden="true" />{{ rule.running ? '检测中' : rule.pending ? '已排队' : !rule.config.enabled ? '已暂停' : labels[rule.lastStatus ?? ''] ?? '等待首次检测' }}</span>
              <span class="text-xs text-cp-text-secondary">下次 {{ rule.config.enabled ? formatDateTime(rule.nextRunAt) : '—' }}</span>
            </button>
            <div class="mt-2 flex gap-1 px-2">
              <BaseIconButton label="立即检测" :disabled="busy || rule.running || rule.pending" @click="mutate(() => runQualityRule({ id: rule.id, revision: rule.revision }))">
                <Play class="size-4" />
              </BaseIconButton>
              <BaseIconButton :label="rule.config.enabled ? '暂停定时检测' : '启用定时检测'" :disabled="busy" @click="toggle(rule)">
                <Pause v-if="rule.config.enabled" class="size-4" /><ClipboardCheck v-else class="size-4" />
              </BaseIconButton>
              <BaseIconButton label="编辑规则" :disabled="busy" @click="edit(rule)">
                <Pencil class="size-4" />
              </BaseIconButton>
              <BaseIconButton label="删除规则" :disabled="busy" @click="remove(rule)">
                <Trash2 class="size-4" />
              </BaseIconButton>
            </div>
          </div>
          <p v-if="!visibleRules.length" class="py-10 text-center text-cp-text-secondary">
            {{ loading ? '加载中…' : '暂无规则' }}
          </p>
        </div>
      </section>
      <section class="min-w-0 xl:border-l xl:border-cp-border xl:pl-6">
        <div v-if="selected" class="mb-5 min-w-0 border-b border-cp-border pb-4">
          <div class="flex min-w-0 items-center justify-between gap-3">
            <p class="min-w-0 truncate text-base font-semibold" :title="accountName(selected.config.accountId)">
              {{ accountName(selected.config.accountId) }}
            </p>
            <span class="shrink-0 text-xs" :class="ruleStatusColor(selected)">{{ selected.running ? '检测中' : selected.pending ? '已排队' : selected.config.enabled ? '定时开启' : '已暂停' }}</span>
          </div>
          <p class="mt-1 truncate font-mono text-xs text-cp-text-secondary" :title="selected.config.model">
            {{ selected.config.model }}
          </p>
          <dl class="mt-4 grid min-w-0 grid-cols-2 gap-x-4 gap-y-3 text-xs sm:grid-cols-3">
            <div>
              <dt class="text-cp-text-secondary">
                每轮次数
              </dt>
              <dd class="mt-1 font-medium tabular-nums">
                {{ selected.config.repetitions }} 次
              </dd>
            </div>
            <div class="min-w-0">
              <dt class="text-cp-text-secondary">
                定时计划
              </dt>
              <dd class="mt-1 break-words font-mono" :title="selected.config.timezone">
                {{ selected.config.cron }}
              </dd>
            </div>
            <div class="col-span-2 min-w-0 sm:col-span-1">
              <dt class="text-cp-text-secondary">
                下次执行
              </dt>
              <dd class="mt-1 tabular-nums">
                {{ selected.config.enabled ? formatDateTime(selected.nextRunAt) : '—' }}
              </dd>
            </div>
          </dl>
        </div>
        <div class="mb-3 flex items-center justify-between gap-3">
          <h2 class="text-base font-semibold">
            检测记录
          </h2>
          <span class="text-xs tabular-nums text-cp-text-secondary">{{ runs.length }} 条</span>
        </div>
        <p v-if="historyError" role="alert" class="text-cp-error">
          {{ historyError }}
        </p>
        <div class="divide-y divide-cp-border sm:hidden">
          <article v-for="run in runs" :key="run.id" class="py-3 text-cp-sm">
            <div class="flex items-center justify-between gap-2">
              <div class="min-w-0">
                <span class="font-medium" :class="statusColor(run.status)">{{ labels[run.status] ?? run.status }}</span>
                <p class="mt-1 text-xs tabular-nums text-cp-text-secondary">
                  {{ formatDateTime(run.startedAt) }}
                </p>
              </div>
              <BaseIconButton label="查看结果" @click="show(run)">
                <Eye class="size-4" />
              </BaseIconButton>
            </div>
            <dl class="mt-3 grid grid-cols-4 gap-2 text-xs">
              <div>
                <dt class="text-cp-text-secondary">
                  通过
                </dt><dd class="mt-1 font-medium tabular-nums">
                  {{ run.correct }}
                </dd>
              </div>
              <div>
                <dt class="text-cp-text-secondary">
                  不符
                </dt><dd class="mt-1 font-medium tabular-nums">
                  {{ run.incorrect }}
                </dd>
              </div>
              <div>
                <dt class="text-cp-text-secondary">
                  不确定
                </dt><dd class="mt-1 font-medium tabular-nums">
                  {{ run.unknown }}
                </dd>
              </div>
              <div>
                <dt class="text-cp-text-secondary">
                  错误
                </dt><dd class="mt-1 font-medium tabular-nums">
                  {{ run.requestErrors }}
                </dd>
              </div>
            </dl>
          </article>
          <p v-if="!runs.length" class="py-12 text-center text-cp-sm text-cp-text-secondary">
            {{ historyLoading ? '加载中…' : '暂无检测记录' }}
          </p>
        </div>
        <div class="hidden overflow-x-auto sm:block">
          <table class="w-full min-w-[620px] text-left text-cp-sm">
            <thead>
              <tr class="border-b border-cp-border text-cp-text-secondary">
                <th class="px-3 pb-3 pt-1 font-medium">
                  时间
                </th><th class="px-3 pb-3 pt-1 font-medium">
                  结果
                </th><th class="px-3 pb-3 pt-1 font-medium">
                  通过 / 不符
                </th><th class="px-3 pb-3 pt-1 font-medium">
                  不确定 / 错误
                </th><th class="px-3 pb-3 pt-1 font-medium">
                  详情
                </th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="run in runs" :key="run.id" class="border-b border-cp-border last:border-b-0">
                <td class="whitespace-nowrap px-3 py-3">
                  {{ formatDateTime(run.startedAt) }}
                </td>
                <td class="whitespace-nowrap px-3 py-3" :class="statusColor(run.status)">
                  {{ labels[run.status] ?? run.status }}
                </td>
                <td class="px-3 py-3 tabular-nums">
                  {{ run.correct }} / {{ run.incorrect }}
                </td>
                <td class="px-3 py-3 tabular-nums">
                  {{ run.unknown }} / {{ run.requestErrors }}
                </td>
                <td class="px-3 py-3">
                  <BaseIconButton label="查看结果" @click="show(run)">
                    <Eye class="size-4" />
                  </BaseIconButton>
                </td>
              </tr>
              <tr v-if="!runs.length">
                <td colspan="5" class="px-3 py-14 text-center text-cp-text-secondary">
                  {{ historyLoading ? '加载中…' : '暂无检测记录' }}
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>
    </div>
    <QualityDrawer v-model="editorOpen" :title="editing ? '编辑检测规则' : '新建检测规则'" :busy="busy">
      <form id="quality-rule-form" class="grid min-w-0 gap-4" @submit.prevent="save">
        <p v-if="editorError || catalogError" role="alert" class="text-cp-error">
          {{ editorError || catalogError }}
        </p>
        <BaseInput v-if="!editing" v-model="accountSearch" aria-label="搜索被测账号" placeholder="搜索账号" />
        <FormItem label="被测账号" required>
          <BaseSelect v-model="draft.accountId" class="min-w-0 w-full" :options="accounts" :disabled="!!editing" aria-label="被测账号" />
        </FormItem>
        <div class="grid gap-4 sm:grid-cols-2">
          <FormItem label="检测模型" required>
            <BaseInput v-model="draft.model" placeholder="模型 ID" />
          </FormItem>
          <FormItem label="推理强度">
            <BaseSelect v-model="effort" :options="[{ value: '', label: '按默认' }, ...efforts]" />
          </FormItem>
          <FormItem label="定时 Cron" required>
            <BaseInput v-model="draft.cron" placeholder="0 */6 * * *" />
          </FormItem>
          <FormItem label="时区" required>
            <BaseInput v-model="draft.timezone" />
          </FormItem>
          <div class="grid gap-2 text-cp-sm">
            <span>每轮次数</span><BaseNumberInput v-model="draft.repetitions" label="每轮次数" :min="1" :max="8" />
          </div>
          <BaseSwitch v-model="draft.enabled" label="启用定时检测" show-label />
        </div>
        <FormItem label="题目" required>
          <BaseTextarea v-model="draft.prompt" :rows="5" />
        </FormItem>
        <FormItem label="参考答案" required>
          <BaseTextarea v-model="draft.referenceAnswer" :rows="3" />
        </FormItem>
        <div class="border-t border-cp-border pt-4">
          <h3 class="mb-3 font-semibold">
            判题配置
          </h3>
          <div class="grid gap-4">
            <BaseInput v-model="groupSearch" aria-label="搜索判题分组" placeholder="搜索分组" />
            <FormItem label="判题分组" required>
              <BaseSelect v-model="draft.judgeGroupId" class="min-w-0 w-full" :options="groups" aria-label="判题分组" />
            </FormItem>
            <FormItem label="判题模型" required>
              <BaseInput v-model="draft.judgeModel" placeholder="模型 ID" />
            </FormItem>
            <FormItem label="判题提示词" required>
              <BaseTextarea v-model="draft.judgePrompt" :rows="3" />
            </FormItem>
          </div>
        </div>
      </form>
      <template #footer>
        <BaseButton :disabled="busy" @click="editorOpen = false">
          取消
        </BaseButton>
        <BaseButton form="quality-rule-form" type="submit" :disabled="busy || !draft.accountId || !draft.judgeGroupId">
          <Save class="size-4" />保存
        </BaseButton>
      </template>
    </QualityDrawer>
    <QualityDrawer v-model="detailOpen" title="检测详情">
      <p v-if="detailError" role="alert" class="text-cp-error">
        {{ detailError }}
      </p>
      <p v-else-if="!detail" class="text-cp-text-secondary">
        加载中…
      </p>
      <div v-else class="grid min-w-0 gap-4">
        <div class="grid gap-1 border-b border-cp-border pb-4 text-cp-sm">
          <span class="break-all">{{ accountName(detail.accountId) }}</span>
          <span class="break-all font-mono">{{ detail.model }}</span>
          <span>{{ formatDateTime(detail.startedAt) }}</span>
          <strong :class="statusColor(detail.status)">{{ labels[detail.status] ?? detail.status }}</strong>
        </div>
        <details v-if="detail.config" class="min-w-0 border-b border-cp-border pb-4 text-cp-sm">
          <summary class="cursor-pointer font-semibold">
            本轮题目与参考答案
          </summary>
          <p class="mt-3 whitespace-pre-wrap break-words [overflow-wrap:anywhere]">
            {{ detail.config.prompt }}
          </p>
          <p class="mt-3 whitespace-pre-wrap break-words [overflow-wrap:anywhere]">
            {{ detail.config.referenceAnswer }}
          </p>
          <p class="mt-3 break-all text-cp-text-secondary">
            判题模型 {{ detail.config.judgeModel }}
          </p>
        </details>
        <section v-for="answer in detail.answers ?? []" :key="answer.index" class="min-w-0 border-b border-cp-border pb-4">
          <div class="mb-2 flex flex-wrap gap-3 text-cp-sm">
            <strong>第 {{ answer.index }} 次</strong><span :class="statusColor(answer.verdict)">{{ labels[answer.verdict] }}</span><span>{{ (answer.elapsedMs / 1000).toFixed(1) }} 秒</span>
          </div>
          <p v-if="answer.returnedModel" class="mb-2 break-all text-xs text-cp-text-secondary">
            返回模型 {{ answer.returnedModel }}
          </p>
          <pre class="max-h-80 overflow-auto whitespace-pre-wrap break-words text-cp-sm [overflow-wrap:anywhere]">{{ answer.answer || '无回答' }}</pre>
          <p class="mt-3 whitespace-pre-wrap break-words text-cp-sm [overflow-wrap:anywhere]">
            {{ answer.reason }}
          </p>
          <p v-if="answer.judgeAccountId" class="mt-2 break-all text-xs text-cp-text-secondary">
            判题账号 {{ accountName(answer.judgeAccountId) }}
          </p>
        </section>
      </div>
    </QualityDrawer>
    <BaseConfirmModal v-model="deleteOpen" title="删除检测规则" destructive :loading="busy" @confirm="confirmDelete">
      删除此账号的检测规则及其历史记录？
    </BaseConfirmModal>
  </div>
</template>

<style scoped>
.quality-header {
  min-height: 44px;
}
.quality-header :deep(h1) {
  font-size: 26px;
  line-height: 1.25;
  letter-spacing: 0;
}
.quality-rule-selected {
  border-inline-start: 3px solid var(--cp-color-primary);
}
</style>
