<script setup lang="ts">
import type { QualityRuleTemplate } from '@/api/modules/quality-ops'
import { onScopeDispose, ref, shallowRef, watch } from 'vue'
import { useRouter } from 'vue-router'
import { applyQualityTemplate, getQualityTemplates } from '@/api/modules/quality-ops'
import BaseButton from '@/components/base/BaseButton.vue'
import { t } from '@/i18n'
import { errorMessage } from '@/utils/async'
import QualityDrawer from '@/views/quality-ops/QualityDrawer.vue'
import { qualityScheduleSummary } from '@/views/quality-ops/schedule'

const props = defineProps<{ accountId: string, accountName: string }>()
const emit = defineEmits<{ applied: [] }>()
const open = defineModel<boolean>({ default: false })
const router = useRouter()
const templates = shallowRef<QualityRuleTemplate[]>([])
const selected = shallowRef<QualityRuleTemplate | null>(null)
const loading = ref(false)
const busy = ref(false)
const error = ref('')
const uncertain = ref(false)
let controller: AbortController | undefined
let disposed = false

async function load() {
  controller?.abort()
  const owner = new AbortController()
  controller = owner
  loading.value = true
  error.value = ''
  selected.value = null
  templates.value = []
  try {
    const result = await getQualityTemplates({ signal: owner.signal, silent: true })
    if (!owner.signal.aborted && !disposed)
      templates.value = result
  }
  catch (cause) {
    if (!owner.signal.aborted && !disposed)
      error.value = errorMessage(cause)
  }
  finally {
    if (controller === owner)
      loading.value = false
  }
}

async function enable() {
  if (busy.value || loading.value || !selected.value || uncertain.value)
    return
  busy.value = true
  error.value = ''
  const target = props.accountId
  const choice = selected.value
  try {
    // Create-only: a rule created concurrently must never be replaced.
    const results = await applyQualityTemplate({ id: choice.id, revision: choice.revision, enable: true, targets: [{ accountId: target, ruleId: null, revision: null }] })
    if (disposed)
      return
    const result = results.find(item => item.accountId === target)
    emit('applied')
    if (result?.success)
      open.value = false
    else
      error.value = result?.message || '规则未创建，请刷新后重试'
  }
  catch (cause) {
    if (!disposed) {
      uncertain.value = true
      error.value = `${errorMessage(cause)}。请关闭后核对账号状态，避免重复提交。`
      emit('applied')
    }
  }
  finally { busy.value = false }
}

function configure() {
  open.value = false
  void router.push({ path: '/quality-ops', query: { accountId: props.accountId, create: '1' } })
}
watch(open, (value) => {
  if (value) {
    uncertain.value = false
    templates.value = []
    void load()
  }
  else {
    controller?.abort()
  }
}, { immediate: true })
onScopeDispose(() => {
  disposed = true
  controller?.abort()
})
</script>

<template>
  <QualityDrawer v-model="open" title="开启质量检测" :busy="busy">
    <div class="grid min-w-0 gap-4 text-cp-sm">
      <p class="break-all">
        为 {{ accountName }} 选择监测模板。首次配置后，即可在账号菜单一键开关。
      </p>
      <p v-if="loading" role="status">
        正在读取监测模板…
      </p>
      <div v-else class="grid gap-2" role="group" aria-label="选择监测模板">
        <BaseButton v-for="template in templates" :key="template.id" :disabled="busy || uncertain" :aria-pressed="selected?.id === template.id" :variant="selected?.id === template.id ? 'primary' : 'secondary'" class="h-auto! justify-start! whitespace-normal! py-3 text-left" @click="selected = template">
          <span class="min-w-0 break-words">
            <span class="block">{{ template.name }}</span>
            <span class="block text-cp-xs">{{ template.config.model }} · {{ qualityScheduleSummary(template.config) }}</span>
          </span>
        </BaseButton>
        <p v-if="!templates.length && !error">
          暂无监测模板，可先手动配置检测规则。
        </p>
      </div>
      <p v-if="selected" class="text-cp-text-secondary">
        {{ selected.config.failureAction === 'apply_account_template' ? `异常时应用：${selected.config.failureTemplate?.config.name ?? '账号模板'}` : selected.config.failureAction === 'disable_scheduling' ? '异常时暂停调度' : selected.config.failureAction === 'remove_groups' ? '异常时移出指定分组' : selected.config.failureAction === 'enable_excel' ? '异常时开启 Excel' : '仅检测，不自动处置' }}；{{ selected.config.autoRestore ? '允许自动恢复原配置' : '不自动恢复原配置' }}。确认后开启定时检测。
      </p>
      <p v-if="error" role="alert" class="break-words text-cp-error">
        {{ error }}
      </p>
      <div class="flex flex-wrap gap-2">
        <BaseButton :disabled="busy || loading || uncertain" @click="load">
          刷新模板
        </BaseButton>
        <BaseButton :disabled="busy" @click="configure">
          手动配置
        </BaseButton>
      </div>
    </div>
    <template #footer>
      <BaseButton :disabled="busy" @click="open = false">
        {{ t('common.actions.cancel') }}
      </BaseButton>
      <BaseButton variant="primary" :disabled="!selected || loading || uncertain" :loading="busy" @click="enable">
        确认开启
      </BaseButton>
    </template>
  </QualityDrawer>
</template>
