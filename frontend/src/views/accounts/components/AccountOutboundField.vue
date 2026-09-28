<script setup lang="ts">
import type { Ipv6EgressConfig } from '@/api/modules/ipv6-egress'
import type { MihomoStatus } from '@/api/modules/mihomo'
import type { RequestProxySource } from '@/utils/request-proxy-source'
import { computed, onScopeDispose, shallowRef, watch } from 'vue'
import { getIpv6Egress, ipv6EgressModes } from '@/api/modules/ipv6-egress'
import { getMihomo } from '@/api/modules/mihomo'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { useProxyCatalog } from '@/composables/useProxyCatalog'

const props = withDefaults(defineProps<{
  accountId?: string
  endpoint?: string | null
  currentSource?: RequestProxySource
  openai?: boolean
  disabled?: boolean
  preserve?: boolean
  error?: string
}>(), { preserve: true })
const mode = defineModel<string>('mode', { required: true })
const proxyId = defineModel<string>('proxyId', { required: true })
const egressMode = defineModel<string>('egressMode', { required: true })
const { proxies, loading } = useProxyCatalog()
const config = shallowRef<Ipv6EgressConfig>()
const configError = shallowRef(false)
const mihomo = shallowRef<MihomoStatus>()
const mihomoError = shallowRef(false)
let mihomoController: AbortController | undefined
let controller: AbortController | undefined
const ipv6Options = ipv6EgressModes.filter(item => item.value !== 'unchanged').map(({ value, label }) => ({ value, label }))
const options = computed(() => [
  ...(mode.value === 'legacy' ? [{ value: 'legacy', label: '保持模板原出口配置' }] : []),
  ...(props.preserve ? [{ value: 'preserve', label: '保持原设置' }] : []),
  ...(props.openai ? [{ value: 'inherit', label: '跟随全局策略' }] : []),
  { value: 'direct', label: '服务器默认直连' },
  { value: 'proxy', label: '指定代理' },
  ...(props.openai
    ? [
        { value: 'mihomo', label: 'Mihomo 会话代理池' },
        { value: 'proxy_pool', label: '普通代理池' },
        { value: 'ipv6', label: 'IPv6 地址池' },
      ]
    : []),
])
const proxyOptions = computed(() => proxies.value.map(proxy => ({
  value: proxy.id,
  label: `${proxy.name}${proxy.lastTest?.success ? '' : proxy.lastTest ? '（测试失败）' : '（未测试）'}`,
  description: proxy.endpoint,
  disabled: proxy.lastTest?.success !== true,
})))
const globalLabel = computed(() => ipv6EgressModes.find(item => item.value === config.value?.defaultMode)?.label)
const currentLabel = computed(() => {
  if (props.currentSource === 'mihomo')
    return 'Mihomo 会话代理池'
  if (props.currentSource === 'proxy_pool')
    return '普通代理池'
  if (props.endpoint)
    return props.endpoint
  if (!props.openai)
    return '服务器默认直连'
  const override = props.accountId && config.value?.accountOverrides[props.accountId]
  return override ? ipv6EgressModes.find(item => item.value === override)?.label : `跟随全局策略（${globalLabel.value ?? '加载中'}）`
})
const fixed = computed(() => props.accountId && config.value?.fixedBindings[props.accountId])
const fixedUnavailable = computed(() => fixed.value && !config.value?.addresses.some(item => item.address === fixed.value && item.enabled))

watch([() => props.openai, () => props.accountId], async ([openai]) => {
  controller?.abort()
  config.value = undefined
  configError.value = false
  if (!openai)
    return
  const owner = new AbortController()
  controller = owner
  try {
    const loaded = await getIpv6Egress({ silent: true, signal: owner.signal })
    if (!owner.signal.aborted)
      config.value = loaded
  }
  catch {
    if (!owner.signal.aborted)
      configError.value = true
  }
}, { immediate: true })
watch([mode, () => props.openai], async ([selection, openai]) => {
  mihomoController?.abort()
  mihomo.value = undefined
  mihomoError.value = false
  if (!openai || selection !== 'mihomo')
    return
  const owner = new AbortController()
  mihomoController = owner
  try {
    const status = await getMihomo({ silent: true, signal: owner.signal })
    if (!owner.signal.aborted)
      mihomo.value = status
  }
  catch {
    if (!owner.signal.aborted)
      mihomoError.value = true
  }
}, { immediate: true })
onScopeDispose(() => {
  controller?.abort()
  mihomoController?.abort()
})

function selectMode(value: string) {
  mode.value = value
  if (value === 'ipv6' && !ipv6Options.some(item => item.value === egressMode.value))
    egressMode.value = 'fixed_ipv6_reuse'
}
</script>

<template>
  <div class="grid gap-3">
    <BaseFormItem label="出站隧道" :error="error">
      <template v-if="$slots.extra" #extra>
        <slot name="extra" />
      </template>
      <BaseSelect :key="disabled ? 'disabled' : 'enabled'" class="w-full" :model-value="mode" :options="options" :disabled="disabled" aria-label="出站隧道" @update:model-value="selectMode" />
    </BaseFormItem>
    <BaseFormItem v-if="mode === 'proxy'" label="指定代理">
      <BaseSelect v-model="proxyId" class="w-full" :options="proxyOptions" :disabled="disabled || loading" placeholder="选择已保存的代理" aria-label="指定代理" />
    </BaseFormItem>
    <BaseFormItem v-if="mode === 'ipv6' && openai" label="IPv6 出口策略">
      <BaseSelect v-model="egressMode" class="w-full" :options="ipv6Options" :disabled="disabled" aria-label="IPv6 出口策略" />
    </BaseFormItem>
    <p v-if="mode === 'preserve' && accountId" class="m-0 break-all text-cp-xs text-cp-text-secondary">
      当前出口：{{ configError && openai && !endpoint && (!currentSource || currentSource === 'account') ? '策略读取失败，原设置保持不变' : currentLabel }}
    </p>
    <p v-if="mode === 'inherit'" class="m-0 text-cp-xs text-cp-text-secondary">
      全局策略：{{ configError ? '读取失败' : globalLabel ?? '加载中' }}
    </p>
    <p v-if="mode === 'mihomo'" class="m-0 text-cp-xs text-cp-text-secondary">
      {{ mihomoError ? 'Mihomo 状态读取失败' : !mihomo ? '读取 Mihomo 状态…' : !mihomo.installed ? 'Mihomo 尚未安装' : !mihomo.running ? 'Mihomo 未运行' : `Mihomo 运行中 · ${mihomo.nodeStates.length} 个节点` }}
    </p>
    <p v-if="mode === 'proxy_pool'" class="m-0 text-cp-xs text-cp-text-secondary">
      {{ loading ? '读取代理列表…' : `已通过测试的代理：${proxies.filter(item => item.lastTest?.success).length}` }}
    </p>
    <p v-if="mode === 'ipv6' && config" class="m-0 text-cp-xs text-cp-text-secondary">
      可用 IPv6：{{ config.addresses.filter(item => item.enabled).length }}
    </p>
    <p v-if="fixed && (mode === 'preserve' || mode === 'ipv6')" class="m-0 break-all font-mono text-cp-xs" :class="fixedUnavailable ? 'text-cp-warning-text' : 'text-cp-text-secondary'">
      历史固定地址：{{ fixed }}{{ fixedUnavailable ? '（不可用，未自动改绑）' : '' }}
    </p>
  </div>
</template>
