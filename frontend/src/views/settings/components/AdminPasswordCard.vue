<script setup lang="ts">
import { LockKeyhole } from '@lucide/vue'
import { ref } from 'vue'
import { useRouter } from 'vue-router'

import { changeAdminPassword } from '@/api'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import FormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import { toast } from '@/components/base/BaseToast'
import { useAuthStore } from '@/stores/modules/auth'

const router = useRouter()
const auth = useAuthStore()
const currentPassword = ref('')
const newPassword = ref('')
const confirmation = ref('')
const saving = ref(false)
const error = ref('')

async function submit() {
  if (saving.value)
    return
  error.value = ''
  if (newPassword.value !== confirmation.value) {
    error.value = '两次新密码不一致'
    return
  }
  if (newPassword.value.trim().length < 12) {
    error.value = '新密码至少需要 12 个字符'
    return
  }
  saving.value = true
  try {
    await changeAdminPassword({
      currentPassword: currentPassword.value,
      newPassword: newPassword.value,
    })
    auth.invalidateSession()
    toast.success('密码已修改，请重新登录')
    await router.replace('/login')
  }
  catch {
    // 请求层显示服务端错误；不保存密码到全局状态或本地存储。
  }
  finally {
    currentPassword.value = ''
    newPassword.value = ''
    confirmation.value = ''
    saving.value = false
  }
}
</script>

<template>
  <BaseCard title="管理员密码">
    <form class="grid max-w-6xl gap-4" @submit.prevent="submit">
      <div class="grid gap-4 md:grid-cols-3">
        <FormItem label="当前密码" required>
          <BaseInput v-model="currentPassword" type="password" autocomplete="current-password" :disabled="saving" />
        </FormItem>
        <FormItem label="新密码" required>
          <BaseInput v-model="newPassword" type="password" autocomplete="new-password" :disabled="saving" />
        </FormItem>
        <FormItem label="确认新密码" required :error="error">
          <BaseInput v-model="confirmation" type="password" autocomplete="new-password" :disabled="saving" />
        </FormItem>
      </div>
      <div class="flex flex-wrap items-center justify-between gap-3">
        <p class="m-0 text-cp-sm text-cp-text-secondary">
          修改后所有后台登录会话将失效，API Key 不受影响
        </p>
        <BaseButton type="submit" variant="secondary" :loading="saving">
          <template #icon>
            <LockKeyhole class="size-4" />
          </template>
          修改密码
        </BaseButton>
      </div>
    </form>
  </BaseCard>
</template>
