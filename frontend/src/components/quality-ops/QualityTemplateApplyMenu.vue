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
import { toast } from '@/components/base/BaseToast'
import { errorMessage } from '@/utils/async'
import { qualityTemplateTargets } from './monitoring'

const props = defineProps<{ accountIds: string[], disabled?: boolean }>()
const emit = defineEmits<{ applied: [accountIds: string[]], applying: [value: boolean] }>()
const router = useRouter()
const open = ref(false)
const loading = ref(false)
const preparing = ref(false)
const applying = ref(false)
const templates = shallowRef<QualityRuleTemplate[]>([])
const chosen = shallowRef<QualityRuleTemplate | null>(null)
const capturedIds = shallowRef<string[]>([])
const menuError = ref('')
const busy = computed(() => applying.value || preparing.value)
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
async function applySelected() {
  if (busy.value || !chosen.value || !capturedIds.value.length)
    return
  previewController?.abort()
  const owner = new AbortController()
  previewController = owner
  preparing.value = true
  try {
    const state = await getQualityMonitoring(capturedIds.value, { silent: true, signal: owner.signal })
    if (disposed || owner.signal.aborted)
      return
    const targets: QualityTemplateTarget[] = qualityTemplateTargets(capturedIds.value, state)
    applying.value = true
    preparing.value = false
    const result: QualityTemplateApplyResult[] = await applyQualityTemplate({ id: chosen.value.id, revision: chosen.value.revision, targets })
    if (disposed)
      return
    const successes = result.filter(item => item.success).map(item => item.accountId)
    const failures = result.length - successes.length
    emit('applied', successes)
    if (failures)
      toast.warning(`监测模板已应用 ${successes.length} 个账号，${failures} 个账号未应用`)
    else
      toast.success(`监测模板已应用 ${successes.length} 个账号`)
  }
  catch (cause) {
    if (!disposed && !owner.signal.aborted) {
      toast.error(`${errorMessage(cause)}。请刷新账号监测状态核对结果，不会自动重复应用。`)
      emit('applied', [])
    }
  }
  finally {
    if (previewController === owner) {
      preparing.value = false
      applying.value = false
    }
  }
}
function choose(template: QualityRuleTemplate) {
  if (busy.value || props.disabled || !props.accountIds.length || props.accountIds.length > 1000)
    return
  chosen.value = template
  capturedIds.value = [...props.accountIds]
  open.value = false
  void applySelected()
}
watch(busy, value => emit('applying', value), { flush: 'sync' })
watch(open, (value) => {
  if (value)
    void load()
  else
    catalogController?.abort()
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
</template>
