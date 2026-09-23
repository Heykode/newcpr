<script setup lang="ts">
import type { NotificationChannels } from '@/api'
import { BellRing, ChevronDown, Mail, Send, Settings2 } from '@lucide/vue'
import { useLocalStorage } from '@vueuse/core'
import { computed, onMounted, reactive, ref } from 'vue'
import { getNotificationChannels, testNotification, updateNotificationChannels } from '@/api'
import BaseButton from '@/components/base/BaseButton.vue'
import FormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseNumberInput from '@/components/base/BaseNumberInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import { toast } from '@/components/base/BaseToast'
import { errorMessage } from '@/utils/async'
import { barkLevelHints, barkLevelOptions } from '@/utils/notification-presentation'

const open = useLocalStorage('cpr.settings.notifications.open', false)
const smtpOpen = useLocalStorage('cpr.settings.notifications.smtp.open', false)
const barkOpen = useLocalStorage('cpr.settings.notifications.bark.open', false)
const loading = ref(false)
const loaded = ref(false)
const saving = ref(false)
const testing = ref<'email' | 'bark' | null>(null)
const testRecipient = ref('')
const smtpConnectionDefaults = { port: 465, security: 'tls' as const }
const form = reactive<NotificationChannels>({
  smtp: { enabled: false, host: '', ...smtpConnectionDefaults, username: null, passwordSet: false, fromName: null, fromEmail: null, password: '' },
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
  { label: 'STARTTLS（常用端口 587）', value: 'starttls' },
  { label: 'SSL/TLS（常用端口 465）', value: 'tls' },
  { label: '无加密', value: 'none' },
]

function apply(data: NotificationChannels, suggestSmtpDefaults = false) {
  Object.assign(form.smtp, data.smtp, { password: '' })
  const smtp = data.smtp
  // Recognize the legacy database seed without replacing configured SMTP settings.
  const unconfigured = !smtp.enabled && !smtp.host && !smtp.username && !smtp.passwordSet && !smtp.fromName && !smtp.fromEmail
  if (suggestSmtpDefaults && unconfigured && smtp.port === 587 && smtp.security === 'starttls')
    Object.assign(form.smtp, smtpConnectionDefaults)
  Object.assign(form.bark, data.bark, { deviceKey: '' })
  form.lastTest = data.lastTest
  form.updatedAt = data.updatedAt
  testRecipient.value ||= data.smtp.fromEmail ?? ''
}

async function load() {
  loading.value = true
  try {
    apply(await getNotificationChannels({ silent: true }), true)
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
  let saved = false
  try {
    saved = await save()
    if (!saved)
      return
    await testNotification({ channel, target: channel === 'email' ? testRecipient.value.trim() : 'default' })
    toast.success(channel === 'email' ? '测试邮件已发送' : 'Bark 测试已发送')
  }
  catch (error) { toast.error(errorMessage(error, '通知测试失败')) }
  finally {
    if (saved)
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
          <Mail class="size-4" /><b class="flex-1">SMTP 发件配置</b><span class="text-cp-xs text-cp-text-tertiary">{{ form.smtp.enabled ? '已启用' : '未启用' }}</span><ChevronDown class="size-4" :class="smtpOpen ? 'rotate-180' : ''" />
        </button>
        <div v-if="smtpOpen" class="mt-3 grid gap-3 md:grid-cols-2">
          <div class="flex items-center gap-2 text-cp-sm md:col-span-2">
            <BaseSwitch v-model="form.smtp.enabled" label="启用邮件通道" />启用邮件通道
          </div>
          <FormItem label="SMTP 服务器地址">
            <BaseInput v-model="form.smtp.host" placeholder="smtp.example.com" />
          </FormItem>
          <FormItem label="SMTP 端口" control-id="notification-smtp-port">
            <BaseNumberInput id="notification-smtp-port" v-model="form.smtp.port" label="SMTP 端口" :min="1" :max="65535" />
          </FormItem>
          <FormItem label="登录账号（可选）">
            <BaseInput v-model="form.smtp.username!" placeholder="user@example.com" autocomplete="off" />
          </FormItem>
          <FormItem label="密码 / 授权码">
            <BaseInput v-model="form.smtp.password!" type="password" autocomplete="new-password" :placeholder="form.smtp.passwordSet ? '已保存，留空保持不变' : '密码、SMTP 授权码或令牌'" />
          </FormItem>
          <FormItem label="发件人地址 (From)">
            <BaseInput v-model="form.smtp.fromEmail!" type="email" placeholder="noreply@example.com" />
          </FormItem>
          <FormItem label="发件人显示名称（可选）">
            <BaseInput v-model="form.smtp.fromName!" placeholder="例如：CPR 预警" />
          </FormItem>
          <FormItem label="加密方式">
            <BaseSelect v-model="form.smtp.security" class="w-full" :options="securityOptions" />
          </FormItem>
          <FormItem label="测试收件地址">
            <BaseInput v-model="testRecipient" type="email" placeholder="ops@example.com" />
          </FormItem>
          <div class="flex justify-end md:col-span-2">
            <BaseButton :loading="testing === 'email'" @click="test('email')">
              <template #icon>
                <Send class="size-4" />
              </template>发送测试邮件
            </BaseButton>
          </div>
        </div>
      </section>
      <section>
        <button class="flex w-full items-center gap-2 py-1 text-left" :aria-expanded="barkOpen" @click="barkOpen = !barkOpen">
          <BellRing class="size-4" /><b class="flex-1">Bark iPhone 提醒</b><span class="text-cp-xs text-cp-text-tertiary">{{ form.bark.enabled ? '已启用' : '未启用' }}</span><ChevronDown class="size-4" :class="barkOpen ? 'rotate-180' : ''" />
        </button>
        <div v-if="barkOpen" class="mt-3 grid gap-3 md:grid-cols-2">
          <div class="flex items-center gap-2 text-cp-sm md:col-span-2">
            <BaseSwitch v-model="form.bark.enabled" label="启用 Bark 通道" />启用 Bark 通道
          </div>
          <FormItem label="Bark Server（服务器地址）">
            <BaseInput v-model="form.bark.serverUrl" placeholder="https://api.day.app" />
          </FormItem>
          <FormItem label="Device Key（设备密钥）">
            <BaseInput v-model="form.bark.deviceKey!" type="password" autocomplete="new-password" :placeholder="form.bark.deviceKeySet ? '已保存，留空保持不变' : '从 Bark App 复制 Device Key'" />
          </FormItem>
          <p class="text-cp-xs text-cp-text-tertiary md:col-span-2">
            服务器地址与 Device Key 分开填写，不要粘贴整条推送链接。
          </p>
          <div class="border-t border-cp-border-secondary pt-3 text-cp-sm font-heavy md:col-span-2">
            默认提醒规则
          </div>
          <FormItem label="提醒等级">
            <BaseSelect v-model="form.bark.level" class="w-full" :options="barkLevelOptions" aria-describedby="notification-bark-level-hint" />
          </FormItem>
          <FormItem label="铃声名称（可选）">
            <BaseInput v-model="form.bark.sound!" placeholder="alarm（留空用 App 默认铃声）" />
          </FormItem>
          <p id="notification-bark-level-hint" class="text-cp-xs text-cp-text-secondary md:col-span-2" aria-live="polite">
            {{ barkLevelHints[form.bark.level] }}
          </p>
          <FormItem label="重要警告音量（0–10）" control-id="notification-bark-volume">
            <BaseNumberInput id="notification-bark-volume" v-model="form.bark.volume" label="重要警告音量" :min="0" :max="10" />
          </FormItem>
          <div class="flex items-center gap-2 text-cp-sm">
            <BaseSwitch v-model="form.bark.call" label="持续响铃约 30 秒" />持续响铃约 30 秒
          </div>
          <p v-if="form.bark.call" class="text-cp-xs text-cp-text-tertiary md:col-span-2">
            持续响铃只延长时长，不会改变静音设置；音量仅对「重要警告」生效。
          </p>
          <div class="flex justify-end md:col-span-2">
            <BaseButton :loading="testing === 'bark'" @click="test('bark')">
              <template #icon>
                <Send class="size-4" />
              </template>发送测试通知
            </BaseButton>
          </div>
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
