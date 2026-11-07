<script setup lang="ts">
import type { RuntimeUnitFields } from '../composables/useRuntimeUnits'
import type { RequestTuning } from '@/api/modules/settings'
import { ChevronDown, Gauge, LocateFixed, Timer, Zap } from '@lucide/vue'
import { computed, onScopeDispose, ref } from 'vue'
import { defaultRequestLocation, probeDirectLocation } from '@/api/modules/proxies'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import RequestLocationFields from '@/components/RequestLocationFields.vue'

defineProps<{ disabled?: boolean, units: RuntimeUnitFields }>()
const maxConcurrentPerAccount = defineModel<string>('maxConcurrentPerAccount', { required: true })
const openaiGuardianReservedConcurrency = defineModel<string>('openaiGuardianReservedConcurrency', { required: true })
const refreshMarginSeconds = defineModel<string>('refreshMarginSeconds', { required: true })
const refreshConcurrency = defineModel<string>('refreshConcurrency', { required: true })
const openaiSessionBindingTtlHours = defineModel<string>('openaiSessionBindingTtlHours', { required: true })
const openaiAccountAffinity = defineModel<string>('openaiAccountAffinity', { required: true })
const requestTuning = defineModel<RequestTuning>('requestTuning', { required: true })
const advancedOpen = ref(false)
const location = computed({
  get: () => requestTuning.value.openaiRequestLocation ?? defaultRequestLocation(),
  set: (value) => { requestTuning.value.openaiRequestLocation = value },
})
const detectingLocation = ref(false)
const locationMessage = ref('')
let locationController: AbortController | undefined
onScopeDispose(() => locationController?.abort())
async function detectLocation() {
  if (detectingLocation.value)
    return
  const controller = new AbortController()
  locationController = controller
  const before = JSON.stringify(requestTuning.value.openaiRequestLocation)
  detectingLocation.value = true
  locationMessage.value = ''
  try {
    const result = await probeDirectLocation({ signal: controller.signal })
    if (controller.signal.aborted)
      return
    if (result.location?.status === 'detected') {
      if (JSON.stringify(requestTuning.value.openaiRequestLocation) !== before) {
        locationMessage.value = '地区已编辑，请重新检测'
        return
      }
      location.value = result.location.location
      requestTuning.value.openaiLocationOverrideEnabled = true
      locationMessage.value = '已填入检测结果，保存设置后生效'
    }
    else {
      locationMessage.value = result.location?.status === 'conflict'
        ? 'IPv4 与 IPv6 的时区不一致，请手动选择'
        : result.location?.status === 'failed' ? result.location.message : result.message
    }
  }
  catch {
    if (!controller.signal.aborted)
      locationMessage.value = '出口位置检测失败'
  }
  finally {
    detectingLocation.value = false
  }
}
type NumericTuningKey = {
  [Key in keyof RequestTuning]: RequestTuning[Key] extends number ? Key : never
}[keyof RequestTuning]
function tuningNumber(key: NumericTuningKey, scale = 1) {
  return computed({
    get: () => String(requestTuning.value[key] / scale),
    set: (value: string) => {
      const parsed = Number(value) * scale
      if (Number.isFinite(parsed))
        requestTuning.value[key] = parsed
    },
  })
}
const tuningValues = {
  maxWaitingPerKey: tuningNumber('maxWaitingPerKey'),
  keyQueueTimeoutSeconds: tuningNumber('keyQueueTimeoutSeconds'),
  maxAccountSwitches: tuningNumber('maxAccountSwitches'),
  maxRequestAttempts: tuningNumber('maxRequestAttempts'),
  websocketMaxRetries: tuningNumber('websocketMaxRetries'),
  websocketFailureThreshold: tuningNumber('websocketFailureThreshold'),
  rateLimitCooldownSeconds: tuningNumber('rateLimitCooldownSeconds'),
  maxRetryAfterSeconds: tuningNumber('maxRetryAfterSeconds'),
  maxWaitingPerAccount: tuningNumber('maxWaitingPerAccount'),
  accountQueueTimeoutSeconds: tuningNumber('accountQueueTimeoutSeconds'),
}
</script>

<template>
  <BaseCard
    title="通用与 Codex 配置"
    description="请求节奏、并发、重试和账号切换为共用参数，Excel 请求继续遵循；原生 Codex 连接参数另列。"
  >
    <BaseForm class="max-w-6xl sm:grid-cols-2">
      <BaseFormItem prop="openaiAccountAffinity" label="OpenAI 账号亲和模式">
        <BaseSegmented
          v-model="openaiAccountAffinity"
          label="OpenAI 账号亲和模式"
          :options="[{ label: '严格', value: 'strict' }, { label: '优先', value: 'preferred' }, { label: '宽松', value: 'relaxed' }]"
        />
      </BaseFormItem>
      <BaseFormItem prop="openaiSessionBindingTtlHours" label="OpenAI 会话账号绑定时长（小时）">
        <BaseInput
          v-model="openaiSessionBindingTtlHours"
          aria-label="OpenAI 会话账号绑定时长（小时）"
          type="number"
          min="1"
          max="720"
          step="1"
        />
      </BaseFormItem>
      <BaseFormItem
        prop="maxConcurrentPerAccount" label="单账号默认最大并发"
        description="账号未单独设置时使用的并发上限"
      >
        <BaseInput
          v-model="maxConcurrentPerAccount"
          aria-label="单账号默认最大并发"
          type="number"
        />
      </BaseFormItem>

      <BaseFormItem
        prop="openaiGuardianReservedConcurrency" label="Guardian 独立并发"
        description="每个账号额外的审批名额，不占普通并发；0 为共用普通并发"
      >
        <BaseInput
          v-model="openaiGuardianReservedConcurrency"
          aria-label="Guardian 独立并发"
          type="number"
          min="0"
          step="1"
        >
          <template #prefix>
            <Gauge class="size-4" />
          </template>
        </BaseInput>
      </BaseFormItem>

      <BaseFormItem
        prop="refreshMarginSeconds" label="提前刷新秒数"
        description="Token 过期前多少秒触发刷新"
      >
        <BaseInput
          v-model="refreshMarginSeconds"
          aria-label="提前刷新秒数"
          type="number"
        >
          <template #prefix>
            <Timer class="size-4" />
          </template>
          <template #suffix>
            秒
          </template>
        </BaseInput>
      </BaseFormItem>

      <BaseFormItem
        prop="refreshConcurrency" label="刷新并发数"
        description="同时刷新 Token 的最大请求数，减小可避免限流"
      >
        <BaseInput
          v-model="refreshConcurrency"
          aria-label="刷新并发数"
          type="number"
        >
          <template #prefix>
            <Zap class="size-4" />
          </template>
        </BaseInput>
      </BaseFormItem>

      <BaseFormItem
        prop="requestIntervalMs" label="请求间隔（秒）"
        description="控制同一账号两次调度之间的最小等待时间"
      >
        <BaseInput
          :model-value="units.intervalSeconds.input.value" aria-label="请求间隔（秒）"
          step="any"
          type="number"
          @update:model-value="units.intervalSeconds.setInput"
        >
          <template #prefix>
            <Timer class="size-4" />
          </template>
        </BaseInput>
      </BaseFormItem>

      <BaseFormItem
        prop="responsesMaxDecompressedBodyBytes" label="请求解压上限（MiB）"
        description="gzip、deflate、zstd 展开后的最大正文，范围 1 字节–256 MiB"
      >
        <BaseInput
          :model-value="units.bodyMiB.input.value" aria-label="请求解压上限（MiB）"
          type="number"
          min="0.00000095367431640625"
          max="256"
          step="any"
          @update:model-value="units.bodyMiB.setInput"
        >
          <template #prefix>
            <Gauge class="size-4" />
          </template>
        </BaseInput>
      </BaseFormItem>
    </BaseForm>

    <div class="mt-5 border-t border-(--cp-border-color) pt-4">
      <div class="flex items-center justify-between gap-3">
        <h3 class="text-sm font-medium text-cp-text-secondary">
          请求位置覆盖
        </h3>
        <BaseSwitch v-model="requestTuning.openaiLocationOverrideEnabled" label="启用位置覆盖" show-label :disabled="disabled" />
      </div>
      <p class="mt-2 text-cp-xs text-cp-text-tertiary">
        覆盖 Codex 请求中的时区与位置字段，账号出站代理的自定义位置优先
      </p>
      <div class="mt-4 grid gap-4">
        <RequestLocationFields v-model="location" :form-prop="requestTuning.openaiLocationOverrideEnabled ? 'requestTuning.openaiRequestLocation' : undefined" :disabled="disabled || !requestTuning.openaiLocationOverrideEnabled" />
        <div>
          <BaseButton size="sm" :loading="detectingLocation" :disabled="disabled" @click="detectLocation">
            <template #icon>
              <LocateFixed :size="16" />
            </template>
            检测直连出口位置
          </BaseButton>
          <p v-if="locationMessage" class="mt-2 text-cp-xs text-cp-text-secondary" role="status">
            {{ locationMessage }}
          </p>
        </div>
      </div>
    </div>

    <div class="mt-5 border-t border-(--cp-border-color) pt-4">
      <h3 class="text-sm font-medium text-cp-text-secondary">
        下游 Key 并发排队
      </h3>
      <BaseForm class="mt-4 max-w-6xl sm:grid-cols-2">
        <BaseFormItem prop="requestTuning.maxWaitingPerKey" label="Key 最大等待人数（0 为关闭）">
          <BaseInput v-model="tuningValues.maxWaitingPerKey.value" aria-label="Key 最大等待人数" type="number" min="0" max="1024" step="1" />
        </BaseFormItem>
        <BaseFormItem prop="requestTuning.keyQueueTimeoutSeconds" label="Key 并发等待秒数">
          <BaseInput v-model="tuningValues.keyQueueTimeoutSeconds.value" aria-label="Key 并发等待秒数" type="number" min="1" max="120" step="1" :disabled="requestTuning.maxWaitingPerKey === 0" />
        </BaseFormItem>
      </BaseForm>
    </div>

    <div class="mt-5 border-t border-(--cp-border-color) pt-4">
      <div class="flex items-center justify-between gap-3">
        <h3 class="text-sm font-medium text-cp-text-secondary">
          账号忙时等待（OpenAI / xAI）
        </h3>
        <BaseSwitch v-model="requestTuning.accountBusyWaitEnabled" label="账号忙时等待（OpenAI / xAI）" />
      </div>
      <BaseForm class="mt-4 max-w-6xl sm:grid-cols-2">
        <BaseFormItem prop="requestTuning.maxWaitingPerAccount" label="每账号等待请求数上限">
          <BaseInput
            v-model="tuningValues.maxWaitingPerAccount.value"
            aria-label="每账号等待请求数上限"
            type="number"
            min="1"
            max="1000"
            step="1"
            :disabled="!requestTuning.accountBusyWaitEnabled"
          />
        </BaseFormItem>
        <BaseFormItem prop="requestTuning.accountQueueTimeoutSeconds" label="账号最长等待（秒）">
          <BaseInput
            v-model="tuningValues.accountQueueTimeoutSeconds.value"
            aria-label="账号最长等待（秒）"
            type="number"
            min="1"
            max="120"
            step="1"
            :disabled="!requestTuning.accountBusyWaitEnabled"
          />
        </BaseFormItem>
      </BaseForm>
    </div>

    <div class="mt-4 grid gap-2">
      <BaseSwitch v-model="requestTuning.switchAccountOnQueueFailure" label="账号排队失败后尝试切换账号" show-label />
      <p class="text-cp-xs text-cp-text-secondary">
        Key 和账号从各自首次入队独立计时，重试不重置账号预算，总请求期限优先。开关默认关闭；开启后，账号队列满或等待超时仅立即重选一次，不再次排队。严格模式只在新账号准入成功后改绑，固定账号和必须跟随根会话的请求不切换。
      </p>
    </div>

    <div class="mt-5 border-t border-(--cp-border-color) pt-4">
      <button
        type="button"
        class="flex w-full items-center justify-between gap-3 text-left text-sm font-medium text-cp-text-secondary"
        :aria-expanded="advancedOpen"
        @click="advancedOpen = !advancedOpen"
      >
        <span>共享重试与原生连接高级参数</span>
        <ChevronDown class="size-4 transition-transform" :class="{ 'rotate-180': advancedOpen }" />
      </button>

      <BaseForm v-if="advancedOpen" class="mt-4 max-w-6xl sm:grid-cols-2">
        <h3 class="m-0 text-sm font-medium text-cp-text-secondary sm:col-span-2">
          共享重试与限流（Codex / Excel）
        </h3>
        <BaseFormItem prop="requestTuning.websocketMaxRetries" label="同账号传输失败重试次数" description="用于 WS 传输恢复和 Excel 可安全重试的首包前错误（含接口 429），范围 0–100；Excel 还受总路由尝试次数限制">
          <BaseInput v-model="tuningValues.websocketMaxRetries.value" aria-label="同账号传输失败重试次数" type="number" />
        </BaseFormItem>
        <BaseFormItem prop="requestTuning.maxAccountSwitches" label="单个请求最多切换账号次数" description="本次请求失败后最多切换到其他账号的次数，范围 0–31">
          <BaseInput v-model="tuningValues.maxAccountSwitches.value" aria-label="单个请求最多切换账号次数" type="number" />
        </BaseFormItem>
        <BaseFormItem prop="requestTuning.maxRequestAttempts" label="单个请求最多路由尝试次数" description="本次请求最多尝试的账号总次数，范围 1–32">
          <BaseInput v-model="tuningValues.maxRequestAttempts.value" aria-label="单个请求最多路由尝试次数" type="number" />
        </BaseFormItem>
        <BaseFormItem label="真实限流冷却时间" description="收到真实限流后等待多久，单位：秒">
          <BaseInput v-model="tuningValues.rateLimitCooldownSeconds.value" aria-label="真实限流冷却时间" type="number" />
        </BaseFormItem>
        <BaseFormItem prop="requestTuning.maxRetryAfterSeconds" label="上游建议等待上限（秒）" description="单次 Retry-After 超过此值时尝试安全换号，无法换号则返回原错误；0 不接受正数等待建议">
          <BaseInput v-model="tuningValues.maxRetryAfterSeconds.value" aria-label="上游建议等待上限（秒）" type="number" min="0" max="4294967295" step="1" />
        </BaseFormItem>
        <h3 class="mb-0 mt-3 text-sm font-medium text-cp-text-secondary sm:col-span-2">
          原生 Codex 上游连接
        </h3>
        <BaseFormItem label="大请求 HTTP 阈值（MiB）">
          <BaseInput :model-value="units.largeRequestMiB.input.value" aria-label="大请求 HTTP 阈值（MiB）" type="number" min="0" max="64" step="any" @update:model-value="units.largeRequestMiB.setInput" />
        </BaseFormItem>
        <BaseFormItem label="WebSocket 连接最长寿命" description="单位：秒">
          <BaseInput :model-value="units.maxAgeSeconds.input.value" step="any" aria-label="WebSocket 连接最长寿命" type="number" @update:model-value="units.maxAgeSeconds.setInput" />
        </BaseFormItem>
        <BaseFormItem label="WebSocket 无数据等待时间" description="单位：秒">
          <BaseInput :model-value="units.idleSeconds.input.value" step="any" aria-label="WebSocket 无数据等待时间" type="number" @update:model-value="units.idleSeconds.setInput" />
        </BaseFormItem>
        <BaseFormItem label="WebSocket 失败保护阈值" description="窗口内连续失败达到此次数后暂停，默认 3 次">
          <BaseInput v-model="tuningValues.websocketFailureThreshold.value" aria-label="WebSocket 失败保护阈值" type="number" />
        </BaseFormItem>
        <BaseFormItem label="WebSocket 失败统计窗口" description="单位：秒">
          <BaseInput :model-value="units.failureWindowSeconds.input.value" step="any" aria-label="WebSocket 失败统计窗口" type="number" @update:model-value="units.failureWindowSeconds.setInput" />
        </BaseFormItem>
        <BaseFormItem label="WebSocket 暂停时长" description="触发失败保护后暂停多久，单位：秒">
          <BaseInput :model-value="units.failureOpenSeconds.input.value" step="any" aria-label="WebSocket 暂停时长" type="number" @update:model-value="units.failureOpenSeconds.setInput" />
        </BaseFormItem>
      </BaseForm>
    </div>
  </BaseCard>
</template>
