<script setup lang="ts">
import type {
  OutboundUserAgentSelection,
  OutboundUserAgentSettings,
} from '@/api/modules/outbound-user-agent'
import { useIntervalFn } from '@vueuse/core'
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import {
  getOutboundUserAgent,
  previewOutboundUserAgent,
  updateOutboundUserAgent,
} from '@/api/modules/outbound-user-agent'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseTextarea from '@/components/base/BaseTextarea.vue'
import { toast } from '@/components/base/BaseToast'
import { errorMessage } from '@/utils/async'

const settings = ref<OutboundUserAgentSettings | null>(null)
const preview = ref<OutboundUserAgentSettings | null>(null)
const useDefault = ref(true)
const custom = ref('')
const loading = ref(false)
const saving = ref(false)
const checking = ref(false)
const error = ref('')
let revision = 0

const busy = computed(() => loading.value || saving.value || checking.value)
const displayedInput = computed({
  get: () => useDefault.value ? settings.value?.defaultUserAgent ?? '' : custom.value,
  set: value => custom.value = value,
})
const selection = computed<OutboundUserAgentSelection>(() => useDefault.value
  ? { mode: 'default' }
  : { mode: 'custom', userAgent: custom.value })

function populate(value: OutboundUserAgentSettings) {
  settings.value = value
  useDefault.value = value.mode === 'default'
  custom.value = value.customUserAgent ?? value.defaultUserAgent
  preview.value = null
}

watch([useDefault, custom], () => {
  error.value = ''
  preview.value = null
})

function validateInput() {
  if (!useDefault.value && !custom.value.trim()) {
    error.value = '请填写完整 UA，或勾选使用默认'
    return false
  }
  return true
}

async function load(reset = false, silent = false) {
  if (busy.value)
    return
  const current = ++revision
  loading.value = true
  try {
    const value = await getOutboundUserAgent({ silent })
    if (current !== revision)
      return
    if (reset || !settings.value)
      populate(value)
    else
      settings.value = value
    error.value = ''
  }
  catch (cause: unknown) {
    if (current === revision && !silent)
      error.value = errorMessage(cause, '出站 UA 设置读取失败，请重试')
  }
  finally {
    if (current === revision)
      loading.value = false
  }
}

async function check() {
  if (busy.value || !validateInput())
    return
  checking.value = true
  const current = ++revision
  error.value = ''
  try {
    const value = await previewOutboundUserAgent(selection.value)
    if (current === revision)
      preview.value = value
  }
  catch (cause: unknown) {
    if (current === revision)
      error.value = errorMessage(cause, 'UA 格式检查失败')
  }
  finally {
    if (current === revision)
      checking.value = false
  }
}

async function save() {
  if (busy.value || !validateInput())
    return
  const current = ++revision
  saving.value = true
  error.value = ''
  try {
    const value = await updateOutboundUserAgent(selection.value)
    if (current !== revision)
      return
    populate(value)
    toast.success('出站设置已保存')
  }
  catch (cause: unknown) {
    if (current === revision)
      error.value = errorMessage(cause, 'UA 保存结果未确认，请刷新实际配置')
  }
  finally {
    if (current === revision)
      saving.value = false
  }
}

onMounted(() => void load(true))
onUnmounted(() => ++revision)
useIntervalFn(() => void load(false, true), 30_000)
</script>

<template>
  <BaseCard title="OpenAI 出站画像">
    <div class="flex flex-wrap items-center justify-between gap-3">
      <BaseCheckbox
        v-model="useDefault"
        label="使用默认 UA，自动更新"
        show-label
        :disabled="busy || !settings"
      />
      <BaseButton :disabled="busy" @click="load(true)">
        刷新实际配置
      </BaseButton>
    </div>

    <BaseFormItem
      class="mt-4"
      label="完整 UA"
      :error="error"
    >
      <BaseTextarea
        v-model="displayedInput"
        aria-label="OpenAI 出站完整 UA"
        :rows="3"
        :maxlength="512"
        :disabled="useDefault || busy || !settings"
        :aria-invalid="Boolean(error)"
        spellcheck="false"
      />
    </BaseFormItem>

    <div class="mt-3 flex flex-wrap gap-3">
      <BaseButton :loading="checking" :disabled="busy || !settings" @click="check">
        检查格式并预览
      </BaseButton>
      <BaseButton variant="primary" :loading="saving" :disabled="busy || !settings" @click="save">
        保存设置
      </BaseButton>
    </div>

    <div v-if="preview" class="mt-4 rounded-cp bg-cp-bg-container p-3 text-xs text-cp-text-secondary">
      <p class="m-0 font-emphasis">
        待保存预览：{{ preview.osType }} {{ preview.osVersion }} · {{ preview.arch }} · 客户端 {{ preview.coreVersion }} · 桌面 {{ preview.desktopVersion }}
      </p>
      <p class="mt-2 mb-0 break-all">
        {{ preview.effectiveUserAgent }}
      </p>
    </div>

    <dl v-if="settings" class="mt-4 grid gap-2 text-xs">
      <dt class="text-cp-text-secondary">
        当前实际生效
      </dt>
      <dd class="m-0 break-all font-mono text-cp-text">
        {{ settings.effectiveUserAgent }}
      </dd>
      <dt class="text-cp-text-secondary">
        配套的桌面辅助请求 UA
      </dt>
      <dd class="m-0 break-all font-mono text-cp-text">
        {{ settings.effectiveDesktopUserAgent }}
      </dd>
    </dl>
  </BaseCard>
</template>
