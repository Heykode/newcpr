<script setup lang="ts">
import type { Ipv6EgressConfig } from '@/api/modules/ipv6-egress'
import { computed, shallowRef, watch } from 'vue'
import { getIpv6Egress, ipv6EgressModes, updateAccountIpv6Egress } from '@/api/modules/ipv6-egress'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'

const props = defineProps<{ accountId: string, disabled?: boolean }>()
const emit = defineEmits<{ saving: [value: boolean] }>()
const config = shallowRef<Ipv6EgressConfig | null>(null)
const selection = shallowRef('inherit')
const confirmOpen = shallowRef(false)
const action = useAsyncAction()
const busy = action.loading
const options = [{ value: 'inherit', label: '继承全局策略', description: '清除账号覆盖，跟随全局设置' }, ...ipv6EgressModes]
const fixed = computed(() => config.value?.fixedBindings[props.accountId])
const unavailable = computed(() => fixed.value
  && !config.value?.addresses.some(address => address.address === fixed.value && address.enabled))
const globalLabel = computed(() => ipv6EgressModes.find(mode => mode.value === config.value?.defaultMode)?.label)
const selectedLabel = computed(() => options.find(option => option.value === selection.value)?.label)

watch(() => props.accountId, async (accountId) => {
  confirmOpen.value = false
  config.value = null
  await action.run(async () => {
    const loaded = await getIpv6Egress()
    if (props.accountId !== accountId)
      return
    config.value = loaded
    selection.value = loaded.accountOverrides[accountId] ?? 'inherit'
  }, { errorText: '账号 IPv6 策略加载失败' })
}, { immediate: true })

async function save() {
  const current = config.value
  if (!current || busy.value || props.disabled)
    return
  emit('saving', true)
  try {
    await action.run(async () => {
      const result = await updateAccountIpv6Egress({
        accountId: props.accountId,
        revision: current.revision,
        mode: selection.value === 'inherit' ? null : selection.value,
      })
      config.value = result.config
      confirmOpen.value = false
      toast.success('IPv6 策略已独立保存；关闭账号编辑框不会撤销')
    }, { errorText: 'IPv6 策略保存失败；请确认已保存的账号未配置代理，或重新打开以刷新版本' })
  }
  finally {
    emit('saving', false)
  }
}
</script>

<template>
  <div class="grid gap-2 rounded-cp border border-cp-border px-3 py-3">
    <BaseFormItem label="IPv6 出口策略" :description="`全局：${globalLabel ?? '加载中'}`">
      <BaseSelect v-model="selection" :options="options" :disabled="disabled || busy || !config" aria-label="账号 IPv6 出口策略" />
    </BaseFormItem>
    <p v-if="fixed" class="m-0 break-all font-mono text-cp-xs" :class="unavailable ? 'text-cp-warning-text' : 'text-cp-text-secondary'">
      历史固定地址：{{ fixed }}{{ unavailable ? '（不可用，未自动改绑）' : '' }}
    </p>
    <p class="m-0 text-cp-xs text-cp-text-tertiary">
      此策略独立提交，保存后不受账号编辑框的“取消”或关闭影响；底部“保存账号设置”不提交本项。
      启用前请先移除并保存账号代理设置，本次提交不会读取上方尚未保存的代理更改。
    </p>
    <p class="m-0 text-cp-xs text-cp-text-tertiary">
      “继承”会清除账号覆盖；“不启用 IPv6 策略”显式保留原直连或代理设置，不跟随全局启用。
      连续会话优先原 WebSocket 连接，新建模式不覆盖严格续接，也不逐帧轮换地址。
    </p>
    <BaseButton variant="secondary" :loading="busy" :disabled="disabled || !config" @click="confirmOpen = true">
      独立保存 IPv6 策略
    </BaseButton>
  </div>
  <BaseConfirmModal
    v-model="confirmOpen"
    title="立即保存 IPv6 策略？"
    description="这会独立提交到服务器。之后关闭或取消账号编辑不会撤销本次保存；未保存的账号设置不参与本次提交。"
    confirm-text="立即保存 IPv6 策略"
    :loading="busy"
    :confirm-disabled="disabled || !config"
    @confirm="save"
  >
    将保存为：{{ selectedLabel }}
  </BaseConfirmModal>
</template>
