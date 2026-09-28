<script setup lang="ts">
import type { QualityBatchResult, QualityEditableField } from './batch-edit'
import type { QualityRule, QualityRuleConfig } from '@/api/modules/quality-ops'
import { Save, Square } from '@lucide/vue'
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { getQualityRules, saveQualityRule } from '@/api/modules/quality-ops'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import FormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseNumberInput from '@/components/base/BaseNumberInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import BaseTextarea from '@/components/base/BaseTextarea.vue'
import { buildQualityPatch, qualityEditableFields, saveQualityBatch } from './batch-edit'
import QualityCatalogPicker from './QualityCatalogPicker.vue'
import QualityDrawer from './QualityDrawer.vue'
import QualitySchedule from './QualitySchedule.vue'

type CatalogLoader = (page: number, search: string, signal: AbortSignal) => Promise<{
  items: { value: string, label: string, description?: string }[]
  total: number
  totalPages: number
}>
const props = defineProps<{
  rules: QualityRule[]
  selectedIds: string[]
  defaultConfig: QualityRuleConfig
  accountName: (id: string) => string
  groupPage: CatalogLoader
  actionGroupPage: CatalogLoader
}>()
const emit = defineEmits<{
  saved: [id: string]
  finished: []
  busy: [value: boolean]
}>()
const open = defineModel<boolean>({ default: false })
const labels: Record<QualityEditableField, string> = {
  enabled: '启用定时检测',
  cron: '检测频率',
  timezone: '时区',
  detectionMode: '检测模式',
  model: '检测模型',
  reasoningEffort: '推理强度（仅题目检测）',
  repetitions: '每轮并行答题次数（仅题目检测）',
  prompt: '题目（仅题目检测）',
  referenceAnswer: '参考答案（仅题目检测）',
  judgeGroupId: '判题分组（仅题目检测）',
  judgeModel: '判题模型（仅题目检测）',
  judgePrompt: '判题提示词（仅题目检测）',
  failureAction: '异常后的处理',
  failureGroupIds: '处置分组（仅移出分组规则）',
  autoRestore: '自动恢复（不适用于开启 Excel）',
  excelFailureThreshold: '连续异常阈值（仅开启 Excel 规则）',
}
const fields = ref<QualityEditableField[]>([])
const draft = ref<QualityRuleConfig>({ ...props.defaultConfig, failureGroupIds: [] })
const pending = ref<string[]>([])
const results = ref<QualityBatchResult[]>([])
const saving = ref(false)
const error = ref('')
const total = ref(0)
const completed = ref(0)
const stopped = ref(false)
const names = ref<Record<string, string>>({})
const effort = computed({
  get: () => draft.value.reasoningEffort ?? '',
  set: (value: string) => { draft.value.reasoningEffort = value || null },
})
let alive = true
let controller: AbortController | undefined

watch(open, (value) => {
  if (!value)
    return
  const first = props.rules.find(rule => props.selectedIds.includes(rule.id))?.config ?? props.defaultConfig
  draft.value = { ...props.defaultConfig, ...first, failureGroupIds: [...first.failureGroupIds] }
  fields.value = []
  pending.value = [...props.selectedIds]
  names.value = Object.fromEntries(props.rules.map(rule => [rule.id, props.accountName(rule.config.accountId)]))
  results.value = []
  total.value = 0
  completed.value = 0
  error.value = ''
  stopped.value = false
})

function selectField(field: QualityEditableField, checked: boolean) {
  fields.value = checked ? [...new Set([...fields.value, field])] : fields.value.filter(value => value !== field)
}

async function save() {
  if (saving.value || !fields.value.length || !pending.value.length)
    return
  const patch = buildQualityPatch(fields.value, draft.value)
  saving.value = true
  emit('busy', true)
  error.value = ''
  stopped.value = false
  total.value = pending.value.length
  completed.value = 0
  controller = new AbortController()
  try {
    await saveQualityBatch([...pending.value], patch, {
      read: () => getQualityRules({ signal: controller?.signal, silent: true }),
      save: value => saveQualityRule(value),
      stopped: () => !alive || stopped.value,
      onResult: (result) => {
        if (!alive)
          return
        completed.value++
        results.value = [...results.value.filter(value => value.id !== result.id), result]
        if (result.success) {
          pending.value = pending.value.filter(id => id !== result.id)
          emit('saved', result.id)
        }
      },
    })
  }
  catch (cause) {
    if (alive)
      error.value = cause instanceof Error ? cause.message : '刷新规则失败，未开始保存'
  }
  finally {
    if (alive) {
      saving.value = false
      emit('busy', false)
      emit('finished')
    }
  }
}
onBeforeUnmount(() => {
  alive = false
  stopped.value = true
  controller?.abort()
})
</script>

<template>
  <QualityDrawer v-model="open" title="批量编辑检测规则" :busy="saving">
    <p class="mb-4 text-cp-sm text-cp-text-secondary">
      仅修改勾选字段；其他配置保留各自的值。逐条保存，成功项不会重复提交。保存会清零所改规则的连续异常计数，不会立即检测。
    </p>
    <p v-if="error" role="alert" class="mb-4 text-cp-error">
      {{ error }}
    </p>
    <div aria-live="polite" class="mb-4 text-cp-sm tabular-nums">
      待处理 {{ pending.length }} 条<span v-if="total"> · 本轮 {{ completed }}/{{ total }}</span>
    </div>
    <fieldset :disabled="saving" class="grid min-w-0 gap-4">
      <legend class="sr-only">
        选择要统一修改的字段
      </legend>
      <div v-for="field in qualityEditableFields" :key="field" class="grid min-w-0 gap-3 border-b border-cp-border pb-4">
        <BaseCheckbox :model-value="fields.includes(field)" :label="`修改${labels[field]}`" show-label @update:model-value="selectField(field, $event)" />
        <template v-if="fields.includes(field)">
          <BaseSwitch v-if="field === 'enabled' || field === 'autoRestore'" v-model="draft[field]" :label="labels[field]" show-label />
          <QualitySchedule v-else-if="field === 'cron'" v-model="draft.cron" />
          <BaseNumberInput v-else-if="field === 'repetitions' || field === 'excelFailureThreshold'" v-model="draft[field]" :label="labels[field]" :min="1" :max="field === 'repetitions' ? 8 : 100" />
          <QualityCatalogPicker v-else-if="field === 'judgeGroupId'" v-model="draft.judgeGroupId" label="判题分组" :load-page="groupPage" />
          <QualityCatalogPicker v-else-if="field === 'failureGroupIds'" v-model:selected-values="draft.failureGroupIds" label="处置分组" multiple :load-page="actionGroupPage" />
          <FormItem v-else :label="labels[field]">
            <BaseSelect v-if="field === 'detectionMode'" v-model="draft.detectionMode" :options="[{ value: 'answer', label: '题目检测' }, { value: 'state_probe', label: '状态探针' }]" />
            <BaseSelect v-else-if="field === 'failureAction'" v-model="draft.failureAction" :options="[{ value: 'none', label: '仅记录结果' }, { value: 'disable_scheduling', label: '暂停此账号调度' }, { value: 'remove_groups', label: '移出指定分组' }, { value: 'enable_excel', label: '开启 Excel 模式' }]" />
            <BaseSelect v-else-if="field === 'reasoningEffort'" v-model="effort" :options="[{ value: '', label: '按默认' }, ...['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'].map(value => ({ value, label: value }))]" />
            <BaseTextarea v-else-if="field === 'prompt' || field === 'referenceAnswer' || field === 'judgePrompt'" v-model="draft[field]" :rows="3" />
            <BaseInput v-else-if="field === 'timezone' || field === 'model' || field === 'judgeModel'" v-model="draft[field]" />
          </FormItem>
        </template>
      </div>
    </fieldset>
    <ul v-if="results.length" class="mt-4 space-y-2 text-cp-sm" aria-label="批量保存结果">
      <li v-for="result in results" :key="result.id" class="break-words" :class="result.success ? 'text-cp-success' : 'text-cp-error'">
        {{ names[result.id] || result.id }}：{{ result.message }}
      </li>
    </ul>
    <template #footer>
      <BaseButton v-if="saving" :disabled="stopped" @click="stopped = true">
        <Square class="size-4" />停止后续保存
      </BaseButton>
      <BaseButton v-else @click="open = false">
        关闭
      </BaseButton>
      <BaseButton :disabled="saving || !fields.length || !pending.length" @click="save">
        <Save class="size-4" />{{ results.some(result => !result.success) ? '重试未完成项' : '保存所选字段' }}
      </BaseButton>
    </template>
  </QualityDrawer>
</template>
