<script setup lang="ts">
import type { RequestTuning } from '@/api/modules/settings'
import { ChevronDown, Gauge, Timer, Zap } from '@lucide/vue'
import { computed, ref } from 'vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'

const maxConcurrentPerAccount = defineModel<string>('maxConcurrentPerAccount', { required: true })
const refreshMarginSeconds = defineModel<string>('refreshMarginSeconds', { required: true })
const refreshConcurrency = defineModel<string>('refreshConcurrency', { required: true })
const requestIntervalMs = defineModel<string>('requestIntervalMs', { required: true })
const requestTuning = defineModel<RequestTuning>('requestTuning', { required: true })
const advancedOpen = ref(false)
type NumericTuningKey = {
  [Key in keyof RequestTuning]: RequestTuning[Key] extends number ? Key : never
}[keyof RequestTuning]
function tuningNumber(key: NumericTuningKey) {
  return computed({
    get: () => String(requestTuning.value[key]),
    set: (value: string) => {
      const parsed = Number(value)
      if (Number.isFinite(parsed))
        requestTuning.value[key] = parsed
    },
  })
}
const tuningValues = {
  maxWaitingPerKey: tuningNumber('maxWaitingPerKey'),
  keyConcurrencyWaitTimeoutSeconds: tuningNumber('keyConcurrencyWaitTimeoutSeconds'),
  maxAccountSwitches: tuningNumber('maxAccountSwitches'),
  maxRequestAttempts: tuningNumber('maxRequestAttempts'),
  websocketMaxRetries: tuningNumber('websocketMaxRetries'),
  websocketMaxAgeMs: tuningNumber('websocketMaxAgeMs'),
  websocketStreamIdleTimeoutMs: tuningNumber('websocketStreamIdleTimeoutMs'),
  websocketFailureThreshold: tuningNumber('websocketFailureThreshold'),
  websocketFailureWindowMs: tuningNumber('websocketFailureWindowMs'),
  websocketFailureOpenDurationMs: tuningNumber('websocketFailureOpenDurationMs'),
  rateLimitCooldownSeconds: tuningNumber('rateLimitCooldownSeconds'),
  accountBusyWaitStickyMaxWaiting: tuningNumber('accountBusyWaitStickyMaxWaiting'),
  accountBusyWaitStickyTimeoutSeconds: tuningNumber('accountBusyWaitStickyTimeoutSeconds'),
  accountBusyWaitFallbackMaxWaiting: tuningNumber('accountBusyWaitFallbackMaxWaiting'),
  accountBusyWaitFallbackTimeoutSeconds: tuningNumber('accountBusyWaitFallbackTimeoutSeconds'),
}
</script>

<template>
  <BaseCard
    title="运行参数"
    description="请求节奏、账号并发和 Token 刷新"
  >
    <BaseForm class="max-w-6xl sm:grid-cols-2">
      <BaseFormItem
        label="单账号默认最大并发"
        description="账号未单独设置时使用的并发上限"
      >
        <BaseInput
          v-model="maxConcurrentPerAccount"
          aria-label="单账号默认最大并发"
          type="number"
        >
          <template #prefix>
            <Gauge class="size-4" />
          </template>
        </BaseInput>
      </BaseFormItem>

      <BaseFormItem
        label="提前刷新秒数"
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
        </BaseInput>
      </BaseFormItem>

      <BaseFormItem
        label="刷新并发数"
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
        label="请求间隔 ms"
        description="控制同一账号两次调度之间的最小等待时间"
      >
        <BaseInput
          v-model="requestIntervalMs"
          aria-label="请求间隔 ms"
          type="number"
        >
          <template #prefix>
            <Timer class="size-4" />
          </template>
        </BaseInput>
      </BaseFormItem>
    </BaseForm>

    <div class="mt-5 border-t border-(--cp-border-color) pt-4">
      <div class="flex items-center justify-between gap-3">
        <h3 class="text-sm font-medium text-cp-text-secondary">
          OpenAI 搜索地区与时区覆盖
        </h3>
        <BaseSwitch v-model="requestTuning.openaiLocationOverrideEnabled" label="OpenAI 搜索地区与时区覆盖" />
      </div>
    </div>

    <div class="mt-5 border-t border-(--cp-border-color) pt-4">
      <h3 class="text-sm font-medium text-cp-text-secondary">
        下游 Key 并发排队
      </h3>
      <BaseForm class="mt-4 max-w-6xl sm:grid-cols-2">
        <BaseFormItem label="Key 最大等待人数（0 为关闭）">
          <BaseInput v-model="tuningValues.maxWaitingPerKey.value" aria-label="Key 最大等待人数" type="number" min="0" max="1024" step="1" />
        </BaseFormItem>
        <BaseFormItem label="Key 并发等待秒数">
          <BaseInput v-model="tuningValues.keyConcurrencyWaitTimeoutSeconds.value" aria-label="Key 并发等待秒数" type="number" min="1" max="600" step="1" :disabled="requestTuning.maxWaitingPerKey === 0" />
        </BaseFormItem>
      </BaseForm>
    </div>

    <div class="mt-5 border-t border-(--cp-border-color) pt-4">
      <div class="flex items-center justify-between gap-3">
        <h3 class="text-sm font-medium text-cp-text-secondary">
          OpenAI 账号忙时等待
        </h3>
        <BaseSwitch v-model="requestTuning.accountBusyWaitEnabled" label="OpenAI 账号忙时等待" />
      </div>
      <BaseForm class="mt-4 max-w-6xl sm:grid-cols-2">
        <BaseFormItem label="原绑定账号等待人数上限">
          <BaseInput
            v-model="tuningValues.accountBusyWaitStickyMaxWaiting.value"
            aria-label="原绑定账号等待人数上限"
            type="number"
            min="1"
            max="1000"
            step="1"
            :disabled="!requestTuning.accountBusyWaitEnabled"
          />
        </BaseFormItem>
        <BaseFormItem label="原绑定账号最长等待（秒）">
          <BaseInput
            v-model="tuningValues.accountBusyWaitStickyTimeoutSeconds.value"
            aria-label="原绑定账号最长等待（秒）"
            type="number"
            min="1"
            max="600"
            step="1"
            :disabled="!requestTuning.accountBusyWaitEnabled"
          />
        </BaseFormItem>
        <BaseFormItem label="全忙时单账号等待人数上限">
          <BaseInput
            v-model="tuningValues.accountBusyWaitFallbackMaxWaiting.value"
            aria-label="全忙时单账号等待人数上限"
            type="number"
            min="1"
            max="1000"
            step="1"
            :disabled="!requestTuning.accountBusyWaitEnabled"
          />
        </BaseFormItem>
        <BaseFormItem label="全忙时最长等待（秒）">
          <BaseInput
            v-model="tuningValues.accountBusyWaitFallbackTimeoutSeconds.value"
            aria-label="全忙时最长等待（秒）"
            type="number"
            min="1"
            max="600"
            step="1"
            :disabled="!requestTuning.accountBusyWaitEnabled"
          />
        </BaseFormItem>
      </BaseForm>
    </div>

    <div class="mt-5 border-t border-(--cp-border-color) pt-4">
      <button
        type="button"
        class="flex w-full items-center justify-between gap-3 text-left text-sm font-medium text-cp-text-secondary"
        :aria-expanded="advancedOpen"
        @click="advancedOpen = !advancedOpen"
      >
        <span>请求与连接高级参数</span>
        <ChevronDown class="size-4 transition-transform" :class="{ 'rotate-180': advancedOpen }" />
      </button>

      <BaseForm v-if="advancedOpen" class="mt-4 max-w-6xl sm:grid-cols-2">
        <BaseFormItem label="同账号传输失败重试次数" description="同一账号传输失败后最多重试次数，范围 0–100">
          <BaseInput v-model="tuningValues.websocketMaxRetries.value" aria-label="同账号传输失败重试次数" type="number" />
        </BaseFormItem>
        <BaseFormItem label="单个请求最多切换账号次数" description="本次请求失败后最多切换到其他账号的次数，范围 0–31">
          <BaseInput v-model="tuningValues.maxAccountSwitches.value" aria-label="单个请求最多切换账号次数" type="number" />
        </BaseFormItem>
        <BaseFormItem label="单个请求最多路由尝试次数" description="本次请求最多尝试的账号总次数，范围 1–32">
          <BaseInput v-model="tuningValues.maxRequestAttempts.value" aria-label="单个请求最多路由尝试次数" type="number" />
        </BaseFormItem>
        <BaseFormItem label="WebSocket 失败后回退普通 HTTP" description="WebSocket 失败时是否尝试普通 HTTP">
          <BaseSwitch v-model="requestTuning.websocketHttpFallbackEnabled" label="允许回退" show-label />
        </BaseFormItem>
        <BaseFormItem label="WebSocket 连接最长寿命" description="单位：毫秒">
          <BaseInput v-model="tuningValues.websocketMaxAgeMs.value" aria-label="WebSocket 连接最长寿命" type="number" />
        </BaseFormItem>
        <BaseFormItem label="WebSocket 无数据等待时间" description="单位：毫秒">
          <BaseInput v-model="tuningValues.websocketStreamIdleTimeoutMs.value" aria-label="WebSocket 无数据等待时间" type="number" />
        </BaseFormItem>
        <BaseFormItem label="WebSocket 失败保护阈值" description="窗口内连续失败达到此次数后暂停，默认 3 次">
          <BaseInput v-model="tuningValues.websocketFailureThreshold.value" aria-label="WebSocket 失败保护阈值" type="number" />
        </BaseFormItem>
        <BaseFormItem label="WebSocket 失败统计窗口" description="单位：毫秒">
          <BaseInput v-model="tuningValues.websocketFailureWindowMs.value" aria-label="WebSocket 失败统计窗口" type="number" />
        </BaseFormItem>
        <BaseFormItem label="WebSocket 暂停时长" description="触发失败保护后暂停多久，单位：毫秒">
          <BaseInput v-model="tuningValues.websocketFailureOpenDurationMs.value" aria-label="WebSocket 暂停时长" type="number" />
        </BaseFormItem>
        <BaseFormItem label="真实限流冷却时间" description="收到真实限流后等待多久，单位：秒">
          <BaseInput v-model="tuningValues.rateLimitCooldownSeconds.value" aria-label="真实限流冷却时间" type="number" />
        </BaseFormItem>
      </BaseForm>
    </div>
  </BaseCard>
</template>
