<script setup lang="ts">
import type { AutoResetPolicy } from '@/api/modules/reset-credits'
import { RefreshCw, Save } from '@lucide/vue'
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { getAutoResetPolicy, saveAutoResetPolicy } from '@/api/modules/reset-credits'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseNumberInput from '@/components/base/BaseNumberInput.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import { formatDateTime } from '@/utils/date'

const props = defineProps<{ accountId: string, native: boolean }>()
const policy = ref<AutoResetPolicy | null>(null)
const enabled = ref(false)
const fiveHour = ref(100)
const sevenDay = ref(100)
const loading = ref(false)
const saving = ref(false)
const error = ref('')
const uncertain = ref(false)
const saved = ref(false)
const valid = computed(() => [fiveHour.value, sevenDay.value].every(n => Number.isFinite(n) && (n === 0 || (n >= 0.1 && n <= 100))))
let controller: AbortController | undefined
let generation = 0
function apply(value: AutoResetPolicy) {
  policy.value = value
  enabled.value = value.config.enabled
  fiveHour.value = value.config.fiveHourUsedMillis / 1000
  sevenDay.value = value.config.sevenDayUsedMillis / 1000
}
async function load() {
  const token = ++generation
  controller?.abort()
  controller = new AbortController()
  loading.value = true
  error.value = ''
  try {
    const value = await getAutoResetPolicy(props.accountId, { signal: controller.signal, silent: true })
    if (token !== generation)
      return
    apply(value)
    uncertain.value = false
  }
  catch {
    if (token === generation)
      error.value = '自动重置设置读取失败'
  }
  finally {
    if (token === generation)
      loading.value = false
  }
}
async function save() {
  if (!policy.value || saving.value || loading.value || !valid.value || uncertain.value)
    return
  const token = generation
  saving.value = true
  saved.value = false
  error.value = ''
  try {
    const value = await saveAutoResetPolicy(props.accountId, policy.value.revision, {
      enabled: enabled.value,
      fiveHourUsedMillis: Math.round(fiveHour.value * 1000),
      sevenDayUsedMillis: Math.round(sevenDay.value * 1000),
    })
    if (token !== generation)
      return
    apply(value)
    saved.value = true
  }
  catch {
    if (token === generation) {
      uncertain.value = true
      error.value = '保存未确认，请刷新设置核对'
    }
  }
  finally {
    if (token === generation)
      saving.value = false
  }
}
watch(() => props.accountId, () => {
  policy.value = null
  saving.value = false
  saved.value = false
  void load()
}, { immediate: true })
onBeforeUnmount(() => {
  generation++
  controller?.abort()
})
</script>

<template>
  <section class="grid min-w-0 gap-3 border-t border-cp-border pt-4" aria-label="自动使用重置卡">
    <div class="flex items-center justify-between gap-2">
      <h3 class="text-cp-sm font-semibold">
        自动使用重置卡
      </h3>
      <BaseIconButton label="刷新自动重置设置" :disabled="loading || saving" @click="load">
        <RefreshCw class="size-4" :class="loading && 'animate-spin'" />
      </BaseIconButton>
    </div>
    <p v-if="error" role="alert" class="text-cp-sm text-cp-error">
      {{ error }}
    </p>
    <template v-if="policy">
      <BaseSwitch v-model="enabled" label="自动重置" show-label :disabled="loading || saving || (!native && !enabled)" />
      <p v-if="!native" class="text-cp-xs text-cp-text-secondary">
        当前非原生 Codex 通道，自动重置不执行。
      </p>
      <div class="flex flex-wrap items-center justify-between gap-2 text-cp-sm">
        <span>5 小时已用阈值（0 为关闭）</span>
        <BaseNumberInput v-model="fiveHour" label="5 小时已用阈值" :min="0" :max="100" :step="0.1" unit="%" :disabled="loading || saving" />
      </div>
      <div class="flex flex-wrap items-center justify-between gap-2 text-cp-sm">
        <span>7 天已用阈值（0 为关闭）</span>
        <BaseNumberInput v-model="sevenDay" label="7 天已用阈值" :min="0" :max="100" :step="0.1" unit="%" :disabled="loading || saving" />
      </div>
      <p v-if="!valid" role="alert" class="text-cp-xs text-cp-error">
        阈值必须为 0 或 0.1% 至 100%。
      </p>
      <div class="grid gap-1 text-cp-xs text-cp-text-secondary" role="status">
        <span v-if="policy.checkedAt">最近检查：{{ formatDateTime(policy.checkedAt) }}</span>
        <span v-if="policy.message" class="break-words">{{ policy.message }}</span>
        <span v-if="saved">设置已保存；已发送的重置请求不受关闭操作影响。</span>
      </div>
      <BaseButton class="justify-self-end" :disabled="!valid || loading || saving || uncertain || (!native && enabled)" @click="save">
        <Save class="size-4" />保存自动重置
      </BaseButton>
    </template>
  </section>
</template>
