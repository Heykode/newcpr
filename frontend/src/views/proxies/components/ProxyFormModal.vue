<script setup lang="ts">
import type { OutboundProxyRecord } from '@/api'
import type { RequestLocation } from '@/api/modules/proxies'
import { Eye, EyeOff, Save, Wifi } from '@lucide/vue'
import { computed, shallowRef, watch } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseNumberInput from '@/components/base/BaseNumberInput.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import RequestLocationFields from '@/components/RequestLocationFields.vue'
import { t } from '@/i18n'
import { proxyLocationMessage } from '../utils/location'

const props = defineProps<{
  proxy: OutboundProxyRecord | null
  saving: boolean
  testing: boolean
}>()
const emit = defineEmits<{
  save: []
  test: []
}>()
const open = defineModel<boolean>({ required: true })
const name = defineModel<string>('name', { required: true })
const proxyUrl = defineModel<string>('proxyUrl', { required: true })
const maxWebsocketConnections = defineModel<number>('maxWebsocketConnections', { required: true })
const autoLocation = defineModel<boolean>('autoLocation', { required: true })
const locationEnabled = defineModel<boolean>('locationEnabled', { required: true })
const location = defineModel<RequestLocation>('location', { required: true })
const showSecret = shallowRef(false)
const busy = computed(() => props.saving || props.testing)
const title = computed(() => props.proxy ? '编辑代理' : '新增代理')
const connectionDescription = computed(() => props.proxy
  ? '留空保留当前连接和认证信息；填写新地址时，请包含所需的用户名和密码。'
  : '支持 HTTP、HTTPS、SOCKS5 和 SOCKS5H，可在地址中包含用户名和密码。')

watch(open, (isOpen) => {
  if (isOpen)
    showSecret.value = false
})
</script>

<template>
  <BaseModal v-model="open" :title="title" size="md" :dismissible="!busy">
    <BaseForm class="grid gap-5">
      <BaseFormItem :label="t('proxies.editor.nameLabel')" required>
        <BaseInput v-model="name" maxlength="100" :disabled="busy" :aria-label="t('proxies.editor.nameLabel')" :placeholder="t('proxies.editor.namePlaceholder')" />
      </BaseFormItem>
      <BaseFormItem label="代理 URL" :required="!proxy" :description="connectionDescription">
        <BaseInput
          v-model="proxyUrl"
          :type="showSecret ? 'text' : 'password'"
          autocomplete="new-password"
          :disabled="busy"
          aria-label="代理 URL"
          :placeholder="proxy?.endpoint ?? 'socks5h://user:password@host:1080'"
        >
          <template #suffix>
            <BaseIconButton :label="showSecret ? '隐藏代理地址' : '显示代理地址'" :disabled="busy" @click="showSecret = !showSecret">
              <EyeOff v-if="showSecret" class="size-4" />
              <Eye v-else class="size-4" />
            </BaseIconButton>
          </template>
        </BaseInput>
      </BaseFormItem>
      <p v-if="proxy?.accountCount && proxyUrl.trim()" class="m-0 text-cp-sm text-cp-warning-text">
        将更新 {{ proxy.accountCount }} 个关联账号的出口。
      </p>
      <BaseSwitch v-model="autoLocation" label="自动出口地区（地区总开关开启时生效）" show-label :disabled="busy" />
      <BaseNumberInput v-model="maxWebsocketConnections" label="代理 WebSocket 总连接上限（0 不限）" :min="0" :max="4294967295" :step="1" :disabled="busy" />
      <p v-if="autoLocation && proxy?.autoLocation" class="m-0 break-words text-cp-sm text-cp-text-secondary">
        {{ proxyLocationMessage(proxy) }}
      </p>
      <BaseSwitch v-model="locationEnabled" label="手动独立地区（地区总开关开启时生效）" show-label :disabled="busy || autoLocation" />
      <RequestLocationFields v-if="locationEnabled" v-model="location" :disabled="busy || autoLocation" />
    </BaseForm>
    <template #footer>
      <BaseButton variant="secondary" :disabled="busy" @click="open = false">
        {{ t('common.actions.cancel') }}
      </BaseButton>
      <BaseButton variant="secondary" :loading="testing" :disabled="saving" @click="emit('test')">
        <template #icon>
          <Wifi class="size-4" />
        </template>
        {{ t('accounts.connectionTest.title') }}
      </BaseButton>
      <BaseButton variant="primary" :loading="saving" :disabled="testing" @click="emit('save')">
        <template #icon>
          <Save class="size-4" />
        </template>
        {{ t('proxies.editor.saveAction') }}
      </BaseButton>
    </template>
  </BaseModal>
</template>
