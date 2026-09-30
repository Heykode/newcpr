<script setup lang="ts">
import type { CaptureConfig, CaptureSettings } from '@/api/modules/request-capture'
import { RefreshCw, Settings2, Trash2 } from '@lucide/vue'
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { clearCapturedErrors, configureRequestCaptures, getCaptureSettings } from '@/api/modules/request-capture'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseNumberInput from '@/components/base/BaseNumberInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'

const settings = ref<CaptureSettings | null>(null)
const draft = ref<CaptureConfig | null>(null)
const open = ref(false)
const loading = ref(false)
const saving = ref(false)
const clearing = ref(false)
const confirmClear = ref(false)
const stopClearing = ref(false)
const clearedRecords = ref(0)
const notice = ref('')
const busy = computed(() => saving.value || clearing.value)
const policies = [{ label: '满了停止采集', value: 'stop' }, { label: '循环覆盖最旧材料', value: 'overwrite' }]
const switchValue = ref(false)
const error = ref('')
const enabled = computed(() => !!settings.value?.config.enabled && !!settings.value.config.globalErrors)
const valid = computed(() => !!draft.value
  && ['stop', 'overwrite'].includes(draft.value.quotaPolicy ?? 'stop')
  && Number.isInteger(draft.value.quotaMib) && draft.value.quotaMib >= 1 && draft.value.quotaMib <= 102400
  && Number.isInteger(draft.value.retentionDays) && draft.value.retentionDays >= 1 && draft.value.retentionDays <= 30)
let alive = true
let controller: AbortController | undefined

async function load() {
  controller?.abort()
  const current = new AbortController()
  controller = current
  loading.value = true
  error.value = ''
  try {
    const result = await getCaptureSettings({ signal: current.signal, silent: true })
    if (alive && !current.signal.aborted) {
      settings.value = result
      switchValue.value = enabled.value
    }
  }
  catch {
    if (alive && !current.signal.aborted)
      error.value = '读取详细采集设置失败'
  }
  finally {
    if (alive && !current.signal.aborted)
      loading.value = false
  }
}
async function save(config: CaptureConfig) {
  if (busy.value)
    return
  saving.value = true
  error.value = ''
  try {
    await configureRequestCaptures(config)
    if (!alive)
      return
    // A saved switch is distinct from a collector paused by quota or storage.
    if (settings.value)
      settings.value.config = { ...config }
    open.value = false
    await load()
  }
  catch {
    if (alive)
      error.value = '保存详细采集设置失败，设置未确认'
  }
  finally {
    if (alive) {
      switchValue.value = enabled.value
      saving.value = false
    }
  }
}
async function clearMaterials() {
  if (busy.value || !confirmClear.value)
    return
  clearing.value = true
  confirmClear.value = false
  stopClearing.value = false
  clearedRecords.value = 0
  notice.value = ''
  error.value = ''
  let cutoffAt: string | undefined
  let complete = false
  let failure = ''
  try {
    while (!complete && !stopClearing.value) {
      if (!alive)
        break
      const result = await clearCapturedErrors({ confirmed: true, ...(cutoffAt ? { cutoffAt } : {}) })
      if (!alive)
        return
      clearedRecords.value += result.removedRecords
      cutoffAt = result.cutoffAt
      complete = result.complete
      if (complete)
        break
    }
  }
  catch {
    failure = '清理结果未确认，可能已清理部分材料。请刷新查看后再操作，不会自动重试。'
  }
  finally {
    if (alive) {
      await load()
      clearing.value = false
      error.value = failure || error.value
      if (!failure)
        notice.value = `${complete ? '清理完成' : '已停止清理'}，已清理 ${clearedRecords.value} 条采集材料。使用明细和普通错误日志保留。`
    }
  }
}
function toggle(value: boolean) {
  if (settings.value && !loading.value && !busy.value) {
    switchValue.value = value
    void save({ ...settings.value.config, enabled: value, globalErrors: value })
  }
}
function edit() {
  if (!settings.value)
    return
  draft.value = { ...settings.value.config, quotaPolicy: settings.value.config.quotaPolicy ?? 'stop' }
  confirmClear.value = false
  notice.value = ''
  open.value = true
}
function selectPolicy(value: string) {
  if (draft.value && (value === 'stop' || value === 'overwrite'))
    draft.value.quotaPolicy = value
}
onMounted(load)
onBeforeUnmount(() => {
  alive = false
  controller?.abort()
})
</script>

<template>
  <div class="flex min-w-0 flex-wrap items-center gap-2" aria-label="详细错误采集设置">
    <BaseSwitch :model-value="switchValue" label="详细错误采集（全局）" show-label :disabled="loading || busy || !settings" @update:model-value="toggle" />
    <BaseIconButton label="采集容量与保留时间" :disabled="loading || busy || !settings" @click="edit">
      <Settings2 class="size-4" />
    </BaseIconButton>
    <BaseIconButton label="刷新采集状态" :disabled="loading || busy" @click="load">
      <RefreshCw class="size-4" />
    </BaseIconButton>
    <span v-if="settings?.storageFault" role="alert" class="text-cp-xs text-cp-error-text">采集存储异常，已停采</span>
    <span v-else-if="enabled && !settings?.globalActive" role="status" class="text-cp-xs text-cp-warning-text">详细采集已暂停，可清理材料、扩大容量或改为循环覆盖后保存</span>
    <span v-if="error" role="alert" class="text-cp-xs text-cp-error-text">{{ error }}</span>
    <BaseModal v-model="open" title="详细错误采集" size="sm" :dismissible="!busy">
      <form v-if="draft" class="grid min-w-0 gap-4" @submit.prevent="valid && !busy && save({ ...draft })">
        <div class="grid gap-2">
          <span class="text-cp-sm">采集文件总容量</span>
          <BaseNumberInput v-model="draft.quotaMib" label="采集文件总容量" unit="MiB" :min="1" :max="102400" :disabled="busy" />
          <span class="text-cp-xs text-cp-text-secondary">已存 {{ settings?.recordCount ?? '未知' }} 条材料，{{ settings?.storedBytes == null ? '容量未知' : `${(settings.storedBytes / 1024 / 1024).toFixed(2)} MiB` }}</span>
        </div>
        <div class="grid gap-2">
          <span class="text-cp-sm">容量满时的采集方案</span>
          <BaseSelect :model-value="draft.quotaPolicy ?? 'stop'" aria-label="容量满时的采集方案" :options="policies" :disabled="busy" @update:model-value="selectPolicy" />
          <p class="m-0 text-cp-xs text-cp-text-secondary">
            {{ draft.quotaPolicy === 'overwrite' ? '空间不足时删除最旧的已保存材料，继续保存新错误。单条材料超过总容量时跳过，不删除旧材料。' : '空间不足时停止采集，保留已保存材料；清理材料或调整容量后可继续采集。' }}
          </p>
        </div>
        <div class="grid gap-2">
          <span class="text-cp-sm">保留天数</span>
          <BaseNumberInput v-model="draft.retentionDays" label="保留天数" unit="天" :min="1" :max="30" :disabled="busy" />
        </div>
        <BaseSwitch v-model="draft.includeMedia" label="保存媒体正文" show-label :disabled="busy" />
        <p class="m-0 text-cp-xs text-cp-warning-text">
          全局设置。采集正文可能包含敏感对话和文件内容，仅管理员可访问。
        </p>
        <div class="grid gap-2 border-t border-cp-border pt-3">
          <BaseButton v-if="!confirmClear && !clearing" variant="soft" :disabled="busy || loading || settings?.storageFault" @click="confirmClear = true">
            <Trash2 class="size-4" />清理采集材料
          </BaseButton>
          <template v-if="confirmClear">
            <p role="alert" class="m-0 text-cp-xs text-cp-warning-text">
              仅永久删除本实例已保存的详细采集正文、媒体及采集索引；不删除使用明细、普通错误日志、账号或其他文件。采集开关不变，清理期间新产生的材料保留。
            </p>
            <div class="flex flex-wrap justify-end gap-2">
              <BaseButton variant="soft" @click="confirmClear = false">
                取消清理
              </BaseButton>
              <BaseButton @click="clearMaterials">
                确认清理
              </BaseButton>
            </div>
          </template>
          <template v-if="clearing">
            <p role="status" class="m-0 text-cp-xs">
              正在清理，已移除 {{ clearedRecords }} 条材料…
            </p>
            <BaseButton variant="soft" :disabled="stopClearing" @click="stopClearing = true">
              {{ stopClearing ? '完成当前批次后停止' : '停止清理' }}
            </BaseButton>
          </template>
          <p v-if="notice" role="status" class="m-0 text-cp-xs text-cp-text-secondary">
            {{ notice }}
          </p>
        </div>
        <p v-if="error" role="alert" class="m-0 text-cp-sm text-cp-error-text">
          {{ error }}
        </p>
        <div class="flex flex-wrap justify-end gap-2">
          <BaseButton variant="soft" :disabled="busy" @click="open = false">
            取消
          </BaseButton>
          <BaseButton type="submit" :loading="saving" :disabled="!valid || clearing || confirmClear">
            保存
          </BaseButton>
        </div>
      </form>
    </BaseModal>
  </div>
</template>
