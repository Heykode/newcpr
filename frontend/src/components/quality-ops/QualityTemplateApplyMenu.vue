<script setup lang="ts">
import type { QualityRuleTemplate, QualityTemplateApplyResult, QualityTemplateTarget } from '@/api/modules/quality-ops'
import { Activity, ChevronDown, RefreshCw, Settings2 } from '@lucide/vue'
import { computed, onScopeDispose, ref, shallowRef, watch } from 'vue'
import { useRouter } from 'vue-router'
import { applyQualityTemplate, getQualityMonitoring, getQualityTemplates } from '@/api/modules/quality-ops'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseMenuItem from '@/components/base/BaseMenuItem.vue'
import BasePopover from '@/components/base/BasePopover.vue'
import { errorMessage } from '@/utils/async'
import QualityDrawer from '@/views/quality-ops/QualityDrawer.vue'
import { qualityTemplateTargets } from './monitoring'

const props = defineProps<{ accountIds: string[], disabled?: boolean }>()
const emit = defineEmits<{ applied: [accountIds: string[]], applying: [value: boolean] }>()
const router = useRouter()
const open = ref(false)
const confirming = ref(false)
const loading = ref(false)
const preparing = ref(false)
const applying = ref(false)
const templates = shallowRef<QualityRuleTemplate[]>([])
const chosen = shallowRef<QualityRuleTemplate | null>(null)
const capturedIds = shallowRef<string[]>([])
const targets = shallowRef<QualityTemplateTarget[]>([])
const results = shallowRef<QualityTemplateApplyResult[]>([])
const menuError = ref('')
const error = ref('')
const uncertain = ref(false)
const ready = ref(false)
const busy = computed(() => applying.value || preparing.value)
const replacements = computed(() => targets.value.filter(target => target.ruleId !== null).length)
const successes = computed(() => results.value.filter(result => result.success).length)
const failures = computed(() => results.value.filter(result => !result.success))
let catalogController: AbortController | undefined
let previewController: AbortController | undefined
let disposed = false

async function load() {
  catalogController?.abort()
  const owner = new AbortController()
  catalogController = owner
  loading.value = true
  menuError.value = ''
  try {
    const result = await getQualityTemplates({ silent: true, signal: owner.signal })
    if (!disposed && !owner.signal.aborted)
      templates.value = result
  }
  catch (cause) {
    if (!disposed && !owner.signal.aborted)
      menuError.value = errorMessage(cause)
  }
  finally {
    if (catalogController === owner)
      loading.value = false
  }
}
async function prepare() {
  if (busy.value || !chosen.value || !capturedIds.value.length)
    return
  previewController?.abort()
  const owner = new AbortController()
  previewController = owner
  preparing.value = true
  ready.value = false
  error.value = ''
  try {
    const state = await getQualityMonitoring(capturedIds.value, { silent: true, signal: owner.signal })
    if (!disposed && !owner.signal.aborted) {
      targets.value = qualityTemplateTargets(capturedIds.value, state)
      ready.value = true
    }
  }
  catch (cause) {
    if (!disposed && !owner.signal.aborted)
      error.value = errorMessage(cause)
  }
  finally {
    if (previewController === owner)
      preparing.value = false
  }
}
function choose(template: QualityRuleTemplate) {
  if (busy.value || props.disabled || !props.accountIds.length || props.accountIds.length > 1000)
    return
  chosen.value = template
  capturedIds.value = [...props.accountIds]
  targets.value = []
  results.value = []
  error.value = ''
  uncertain.value = false
  open.value = false
  confirming.value = true
  void prepare()
}
async function apply() {
  if (busy.value || !ready.value || !chosen.value)
    return
  applying.value = true
  ready.value = false
  const selection = { id: chosen.value.id, revision: chosen.value.revision, targets: [...targets.value] }
  try {
    const result = await applyQualityTemplate(selection)
    if (disposed)
      return
    results.value = result
    emit('applied', result.filter(item => item.success).map(item => item.accountId))
  }
  catch (cause) {
    if (!disposed) {
      error.value = `${errorMessage(cause)}。请刷新账号监测状态核对结果，不会自动重复应用。`
      uncertain.value = true
      emit('applied', [])
    }
  }
  finally { applying.value = false }
}
function reviewFailures() {
  if (busy.value || !failures.value.length)
    return
  capturedIds.value = failures.value.map(item => item.accountId)
  results.value = []
  targets.value = []
  void prepare()
}
watch(busy, value => emit('applying', value), { flush: 'sync' })
watch(open, (value) => {
  if (value)
    void load()
  else
    catalogController?.abort()
})
watch(confirming, (value) => {
  if (!value) {
    previewController?.abort()
    ready.value = false
  }
})
onScopeDispose(() => {
  disposed = true
  catalogController?.abort()
  previewController?.abort()
})
</script>

<template>
  <BasePopover v-model="open" class="w-full xl:w-auto" :disabled="disabled || busy">
    <template #trigger>
      <BaseButton class="w-full whitespace-nowrap xl:w-auto" :disabled="disabled" :loading="busy" aria-label="监测模板">
        <Activity class="size-4" />监测模板<ChevronDown class="size-3.5" />
      </BaseButton>
    </template>
    <div class="w-80 max-w-[calc(100vw-16px)] p-2" aria-label="监测模板菜单">
      <div class="flex items-center justify-between gap-2 px-3 py-2 text-cp-sm text-cp-text-secondary">
        <span>已选 {{ accountIds.length }} 个账号</span>
        <BaseIconButton label="刷新监测模板" :loading="loading" :disabled="busy" @click="load">
          <RefreshCw class="size-4" />
        </BaseIconButton>
      </div>
      <p v-if="menuError" class="mx-3 my-2 break-words text-cp-sm text-cp-error" role="alert">
        {{ menuError }}
      </p>
      <p v-if="accountIds.length > 1000" class="mx-3 my-2 text-cp-sm text-cp-warning">
        每次最多应用 1000 个账号
      </p>
      <p v-if="loading || (!templates.length && !menuError)" class="mx-3 my-2 text-cp-sm text-cp-text-secondary">
        {{ loading ? '加载中…' : '暂无规则模板' }}
      </p>
      <div class="max-h-64 overflow-y-auto">
        <BaseMenuItem v-for="template in templates" :key="template.id" :disabled="loading || !!menuError || busy || !accountIds.length || accountIds.length > 1000" :aria-label="`应用监测模板：${template.name}`" :title="template.name" class="py-2" @click="choose(template)">
          <span class="block truncate">{{ template.name }}</span>
          <span class="mt-1 block whitespace-normal break-words text-cp-xs leading-normal text-cp-text-secondary">{{ template.config.detectionMode === 'state_probe' ? '状态探针' : '题目检测' }} · {{ template.config.model }} · {{ template.config.enabled ? '定时开启' : '监测暂停' }}</span>
        </BaseMenuItem>
      </div>
      <div class="mt-2 border-t border-cp-border pt-2">
        <BaseMenuItem @click="open = false; router.push({ path: '/quality-ops', query: { tab: 'templates' } })">
          <template #icon>
            <Settings2 class="size-4" />
          </template>
          管理规则模板
        </BaseMenuItem>
      </div>
    </div>
  </BasePopover>
  <QualityDrawer v-model="confirming" title="应用监测模板" :busy="applying">
    <div v-if="chosen" class="grid min-w-0 gap-4 text-cp-sm">
      <h3 class="break-words text-base font-semibold [overflow-wrap:anywhere]">
        {{ chosen.name }}
      </h3>
      <p class="break-all text-cp-text-secondary">
        {{ chosen.config.model }} · {{ chosen.config.enabled ? '启用定时检测' : '保存为暂停状态' }} · {{ chosen.config.cron }} · {{ chosen.config.timezone }}
      </p>
      <p v-if="preparing" role="status">
        正在读取所选账号的监测规则…
      </p>
      <div v-else-if="ready" class="grid gap-3">
        <p>将为 {{ targets.length - replacements }} 个账号新建规则，替换 {{ replacements }} 个账号的已有规则。</p>
        <p class="text-cp-text-secondary">
          已有检测历史保留。按模板计划执行，不立即检测；不修改账号当前的请求出口或调度设置。后续仅在满足规则异常条件时执行配置的处置。
        </p>
      </div>
      <div v-if="results.length" class="grid gap-3" role="status">
        <p>已应用 {{ successes }} 个账号，{{ failures.length }} 个账号未应用。</p>
        <div v-if="failures.length" class="max-h-64 divide-y divide-cp-border overflow-y-auto">
          <p v-for="failure in failures" :key="failure.accountId" class="break-words py-2 text-cp-error [overflow-wrap:anywhere]">
            {{ failure.accountId }}：{{ failure.message }}
          </p>
        </div>
      </div>
      <p v-if="error" class="break-words text-cp-error" role="alert">
        {{ error }}
      </p>
    </div>
    <template #footer>
      <BaseButton :disabled="applying" @click="confirming = false">
        {{ results.length || uncertain ? '完成' : '取消' }}
      </BaseButton>
      <BaseButton v-if="failures.length && !ready" :disabled="busy" @click="reviewFailures">
        重新核对未成功项
      </BaseButton>
      <BaseButton v-else-if="error && !uncertain && !ready" :disabled="busy" @click="prepare">
        重新读取
      </BaseButton>
      <BaseButton v-else-if="!results.length && !uncertain" variant="primary" :disabled="!ready || busy" :loading="applying" @click="apply">
        确认应用
      </BaseButton>
    </template>
  </QualityDrawer>
</template>
