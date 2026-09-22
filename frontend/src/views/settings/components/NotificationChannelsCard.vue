<script setup lang="ts">
import type { NotificationChannels } from '@/api'
import { BellRing, ChevronDown, Mail, Send, Settings2 } from '@lucide/vue'
import { useLocalStorage } from '@vueuse/core'
import { computed, onMounted, reactive, ref } from 'vue'
import { getNotificationChannels, testNotification, updateNotificationChannels } from '@/api'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseNumberInput from '@/components/base/BaseNumberInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import { toast } from '@/components/base/BaseToast'
import { errorMessage } from '@/utils/async'

const open = useLocalStorage('cpr.settings.notifications.open', false)
const smtpOpen = useLocalStorage('cpr.settings.notifications.smtp.open', false)
const barkOpen = useLocalStorage('cpr.settings.notifications.bark.open', false)
const loading = ref(false)
const loaded = ref(false)
const saving = ref(false)
const testing = ref<'email' | 'bark' | null>(null)
const testRecipient = ref('')
const form = reactive<NotificationChannels>({
  smtp: { enabled: false, host: '', port: 587, security: 'starttls', username: null, passwordSet: false, fromName: null, fromEmail: null, password: '' },
  bark: { enabled: false, serverUrl: 'https://api.day.app', deviceKeySet: false, level: 'active', sound: null, volume: 5, call: false, deviceKey: '' },
  lastTest: null,
  updatedAt: '',
})
const lastTest = computed(() => {
  const test = form.lastTest
  if (!test)
    return '尚未测试'
  const status = { sent: '成功', failed: '失败', sending: '发送中', pending: '等待发送' }[test.status]
  return `${test.channel === 'email' ? 'SMTP' : 'Bark'} 测试${status} · ${new Date(test.finishedAt ?? test.createdAt).toLocaleString()}`
})

const securityOptions = [
  { label: 'STARTTLS', value: 'starttls' },
  { label: 'TLS', value: 'tls' },
  { label: '无加密', value: 'none' },
]
const levelOptions = [
  { label: '普通提醒', value: 'active' },
  { label: '时效提醒', value: 'timeSensitive' },
  { label: '重要警告', value: 'critical' },
  { label: '静默记录', value: 'passive' },
]

function apply(data: NotificationChannels) {
  Object.assign(form.smtp, data.smtp, { password: '' })
  Object.assign(form.bark, data.bark, { deviceKey: '' })
  form.lastTest = data.lastTest
  form.updatedAt = data.updatedAt
  testRecipient.value ||= data.smtp.fromEmail ?? ''
}

async function load() {
  loading.value = true
  try {
    apply(await getNotificationChannels({ silent: true }))
    loaded.value = true
  }
  catch (error) {
    toast.error(errorMessage(error, '通知渠道读取失败'))
  }
  finally {
    loading.value = false
  }
}

async function save(): Promise<boolean> {
  if (saving.value || !loaded.value)
    return false
  saving.value = true
  try {
    apply(await updateNotificationChannels(form))
    toast.success('通知渠道已保存')
    return true
  }
  catch (error) {
    toast.error(errorMessage(error, '通知渠道保存失败'))
    return false
  }
  finally {
    saving.value = false
  }
}

async function test(channel: 'email' | 'bark') {
  if (testing.value || saving.value || !loaded.value)
    return
  if (channel === 'email' && !testRecipient.value.trim()) {
    toast.error('请填写测试收件人')
    return
  }
  testing.value = channel
  try {
    if (!await save())
      return
    await testNotification({ channel, target: channel === 'email' ? testRecipient.value.trim() : 'default' })
    toast.success(channel === 'email' ? '测试邮件已发送' : 'Bark 测试已发送')
  }
  catch (error) { toast.error(errorMessage(error, '通知测试失败')) }
  finally {
    await load()
    testing.value = null
  }
}

onMounted(() => {
  if (location.hash === '#notifications')
    open.value = true
  void load()
})
</script>

<template>
  <section id="notifications" class="overflow-hidden rounded-cp border border-cp-border bg-cp-bg-container">
    <button class="flex min-h-14 w-full items-center gap-3 px-4 text-left" :aria-expanded="open" @click="open = !open">
      <Settings2 class="size-4 text-cp-text-secondary" />
      <div class="min-w-0 flex-1">
        <div class="font-heavy text-cp-text">
          通知渠道
        </div>
        <div class="flex flex-wrap gap-x-3 text-cp-xs text-cp-text-tertiary">
          <span>SMTP {{ form.smtp.enabled && form.smtp.host && form.smtp.fromEmail ? '已配置' : '未配置' }} · Bark {{ form.bark.enabled && form.bark.deviceKeySet ? '已配置' : '未配置' }}</span>
          <span>{{ lastTest }}</span>
        </div>
      </div>
      <ChevronDown class="size-4 transition-transform" :class="open ? 'rotate-180' : ''" />
    </button>
    <fieldset v-if="open" class="min-w-0 space-y-3 border-t border-cp-border p-4" :disabled="loading || !loaded || saving || testing !== null">
      <section class="border-b border-cp-border-secondary pb-3">
        <button class="flex w-full items-center gap-2 py-1 text-left" :aria-expanded="smtpOpen" @click="smtpOpen = !smtpOpen">
          <Mail class="size-4" /><b class="flex-1">邮件 SMTP</b><span class="text-cp-xs text-cp-text-tertiary">{{ form.smtp.enabled ? '已启用' : '未启用' }}</span><ChevronDown class="size-4" :class="smtpOpen ? 'rotate-180' : ''" />
        </button>
        <div v-if="smtpOpen" class="mt-3 grid gap-3 md:grid-cols-2">
          <div class="flex items-center gap-2 text-cp-sm">
            <BaseSwitch v-model="form.smtp.enabled" label="启用 SMTP" />启用 SMTP
          </div>
          <BaseSelect v-model="form.smtp.security" :options="securityOptions" aria-label="SMTP 安全模式" />
          <BaseInput v-model="form.smtp.host" placeholder="SMTP 主机" />
          <div class="flex flex-wrap items-center justify-between gap-2 text-cp-sm">
            SMTP 端口<BaseNumberInput v-model="form.smtp.port" label="SMTP 端口" :min="1" :max="65535" />
          </div>
          <BaseInput v-model="form.smtp.username!" placeholder="用户名（可选）" />
          <BaseInput v-model="form.smtp.password!" type="password" :placeholder="form.smtp.passwordSet ? '已保存，留空保持不变' : '密码'" />
          <BaseInput v-model="form.smtp.fromName!" placeholder="发件人名称" />
          <BaseInput v-model="form.smtp.fromEmail!" placeholder="发件邮箱" />
          <BaseInput v-model="testRecipient" placeholder="测试收件人" />
          <BaseButton :loading="testing === 'email'" @click="test('email')">
            <template #icon>
              <Send class="size-4" />
            </template>发送测试邮件
          </BaseButton>
        </div>
      </section>
      <section>
        <button class="flex w-full items-center gap-2 py-1 text-left" :aria-expanded="barkOpen" @click="barkOpen = !barkOpen">
          <BellRing class="size-4" /><b class="flex-1">Bark 推送</b><span class="text-cp-xs text-cp-text-tertiary">{{ form.bark.enabled ? '已启用' : '未启用' }}</span><ChevronDown class="size-4" :class="barkOpen ? 'rotate-180' : ''" />
        </button>
        <div v-if="barkOpen" class="mt-3 grid gap-3 md:grid-cols-2">
          <div class="flex items-center gap-2 text-cp-sm">
            <BaseSwitch v-model="form.bark.enabled" label="启用 Bark" />启用 Bark
          </div>
          <BaseSelect v-model="form.bark.level" :options="levelOptions" aria-label="Bark 提醒等级" />
          <BaseInput v-model="form.bark.serverUrl" placeholder="https://api.day.app" />
          <BaseInput v-model="form.bark.deviceKey!" type="password" :placeholder="form.bark.deviceKeySet ? '已保存，留空保持不变' : 'Device Key'" />
          <BaseInput v-model="form.bark.sound!" placeholder="铃声名称（可选）" />
          <div class="flex flex-wrap items-center justify-between gap-2 text-cp-sm">
            重要警告音量<BaseNumberInput v-model="form.bark.volume" label="重要警告音量" :min="0" :max="10" />
          </div>
          <div class="flex items-center gap-2 text-cp-sm">
            <BaseSwitch v-model="form.bark.call" label="持续响铃约 30 秒" />持续响铃约 30 秒
          </div>
          <BaseButton :loading="testing === 'bark'" @click="test('bark')">
            <template #icon>
              <Send class="size-4" />
            </template>发送测试通知
          </BaseButton>
        </div>
      </section>
      <div class="flex justify-end">
        <BaseButton variant="primary" :loading="saving" :disabled="loading" @click="save">
          保存通知渠道
        </BaseButton>
      </div>
    </fieldset>
  </section>
</template>
