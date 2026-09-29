<script setup lang="ts">
import type { CaptureConfig, CaptureSettings } from '@/api/modules/request-capture'
import { RefreshCw, Settings2 } from '@lucide/vue'
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { configureRequestCaptures, getCaptureSettings } from '@/api/modules/request-capture'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseNumberInput from '@/components/base/BaseNumberInput.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'

const settings = ref<CaptureSettings | null>(null)
const draft = ref<CaptureConfig | null>(null)
const open = ref(false)
const loading = ref(false)
const saving = ref(false)
const switchValue = ref(false)
const error = ref('')
const enabled = computed(() => !!settings.value?.config.enabled && !!settings.value.config.globalErrors)
const valid = computed(() => !!draft.value
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
  if (saving.value)
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
function toggle(value: boolean) {
  if (settings.value && !loading.value && !saving.value) {
    switchValue.value = value
    void save({ ...settings.value.config, enabled: value, globalErrors: value })
  }
}
function edit() {
  if (!settings.value)
    return
  draft.value = { ...settings.value.config }
  open.value = true
}
onMounted(load)
onBeforeUnmount(() => {
  alive = false
  controller?.abort()
})
</script>

<template>
  <div class="flex min-w-0 flex-wrap items-center gap-2" aria-label="详细错误采集设置">
    <BaseSwitch :model-value="switchValue" label="详细错误采集（全局）" show-label :disabled="loading || saving || !settings" @update:model-value="toggle" />
    <BaseIconButton label="采集容量与保留时间" :disabled="loading || saving || !settings" @click="edit">
      <Settings2 class="size-4" />
    </BaseIconButton>
    <BaseIconButton label="刷新采集状态" :disabled="loading || saving" @click="load">
      <RefreshCw class="size-4" />
    </BaseIconButton>
    <span v-if="settings?.storageFault" role="alert" class="text-cp-xs text-cp-error-text">采集存储异常，已停采</span>
    <span v-else-if="enabled && !settings?.globalActive" role="status" class="text-cp-xs text-cp-warning-text">详细采集已暂停，请检查容量后重新保存</span>
    <span v-if="error" role="alert" class="text-cp-xs text-cp-error-text">{{ error }}</span>
    <BaseModal v-model="open" title="详细错误采集" size="sm">
      <form v-if="draft" class="grid min-w-0 gap-4" @submit.prevent="valid && save({ ...draft })">
        <div class="grid gap-2">
          <span class="text-cp-sm">采集文件总容量</span>
          <BaseNumberInput v-model="draft.quotaMib" label="采集文件总容量" unit="MiB" :min="1" :max="102400" :disabled="saving" />
        </div>
        <div class="grid gap-2">
          <span class="text-cp-sm">保留天数</span>
          <BaseNumberInput v-model="draft.retentionDays" label="保留天数" unit="天" :min="1" :max="30" :disabled="saving" />
        </div>
        <BaseSwitch v-model="draft.includeMedia" label="保存媒体正文" show-label :disabled="saving" />
        <p class="m-0 text-cp-xs text-cp-warning-text">
          全局设置。采集正文可能包含敏感对话和文件内容，仅管理员可访问。
        </p>
        <p v-if="error" role="alert" class="m-0 text-cp-sm text-cp-error-text">
          {{ error }}
        </p>
        <div class="flex flex-wrap justify-end gap-2">
          <BaseButton variant="soft" :disabled="saving" @click="open = false">
            取消
          </BaseButton>
          <BaseButton type="submit" :loading="saving" :disabled="!valid">
            保存
          </BaseButton>
        </div>
      </form>
    </BaseModal>
  </div>
</template>
