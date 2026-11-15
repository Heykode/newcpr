<script setup lang="ts">
import { storeToRefs } from 'pinia'
import { computed, shallowRef } from 'vue'
import { useRouter } from 'vue-router'
import AppLanguageMenu from '@/components/AppLanguageMenu.vue'

import { t } from '@/i18n'
import { useAuthStore } from '@/stores/modules/auth'
import { useThemeStore } from '@/stores/modules/theme'

import LoginBackground from './components/LoginBackground.vue'
import LoginPanel from './components/LoginPanel.vue'

const router = useRouter()
const authStore = useAuthStore()
const themeStore = useThemeStore()
const { effectiveTheme } = storeToRefs(themeStore)
const { toggleTheme } = themeStore

const username = shallowRef('')
const password = shallowRef('')
const loginPending = shallowRef(false)
const canSubmit = computed<boolean>(() => !!username.value.trim() && !!password.value.trim())
const loginLoading = computed<boolean>(() => authStore.loading || loginPending.value)
const submitDisabled = computed<boolean>(() => loginLoading.value || !canSubmit.value)

async function handleSubmit(): Promise<void> {
  if (!canSubmit.value || loginPending.value) {
    return
  }

  loginPending.value = true
  const success = await authStore.login({
    username: username.value.trim(),
    password: password.value,
  })

  if (!success) {
    loginPending.value = false
    return
  }

  try {
    await router.push('/')
  }
  finally {
    // 成功时登录页会卸载；导航失败并停留当前页时才恢复按钮。
    if (router.currentRoute.value.path === '/login') {
      loginPending.value = false
    }
  }
}
</script>

<template>
  <main class="login-page relative isolate min-h-dvh overflow-hidden text-(--cp-login-title-color)">
    <LoginBackground />
    <div class="absolute right-4 top-4 z-10">
      <AppLanguageMenu />
    </div>

    <section
      class="grid min-h-dvh items-center justify-items-center px-5 py-16 min-[980px]:justify-items-end min-[980px]:py-[clamp(24px,5dvh,64px)] min-[980px]:pr-[clamp(48px,9vw,180px)] max-[560px]:px-4.5"
      :aria-label="t('auth.pageLabel')"
    >
      <LoginPanel
        v-model:username="username"
        v-model:password="password"
        :loading="loginLoading"
        :submit-disabled="submitDisabled"
        :effective-theme="effectiveTheme"
        @submit="handleSubmit"
        @toggle-theme="toggleTheme"
      />
    </section>
  </main>
</template>

<style scoped>
.login-page {
  --cp-login-panel-line-color: color-mix(in srgb, var(--cp-color-primary-border) 54%, transparent);
  --cp-login-logo-color: var(--cp-color-primary-text);
  --cp-login-title-color: var(--cp-color-text);
  --cp-login-brand-title-color: var(--cp-color-text-heading);
  --cp-login-brand-caption-color: var(--cp-color-text-secondary);
  --cp-login-description-color: color-mix(in srgb, var(--cp-color-text-secondary) 82%, var(--cp-color-text));
  --cp-login-label-color: color-mix(in srgb, var(--cp-color-text) 84%, var(--cp-color-text-secondary));
  --cp-login-input-bg: color-mix(in srgb, var(--cp-input-bg) 82%, var(--cp-color-bg-container));
  --cp-login-input-hover-bg: color-mix(in srgb, var(--cp-input-hover-bg) 86%, var(--cp-color-bg-container));
  --cp-login-input-active-bg: var(--cp-input-active-bg);
  --cp-login-input-icon-color: var(--cp-color-primary-text);
  --cp-login-placeholder-color: var(--cp-color-text-secondary);
  --cp-login-error-bg: var(--cp-color-error-container);
  --cp-login-error-icon-color: var(--cp-color-error);
  --cp-login-error-text-color: var(--cp-color-error-text);
  --cp-login-button-shadow-color: color-mix(in srgb, var(--cp-color-primary) 30%, transparent);
  --cp-login-toggle-bg: color-mix(in srgb, var(--cp-color-fill-tertiary) 90%, var(--cp-color-bg-container));
  --cp-login-toggle-bg-hover: var(--cp-color-fill-tertiary);
  --cp-login-toggle-sun-color: var(--cp-login-logo-color);
  --cp-login-toggle-moon-color: var(--cp-color-text-secondary);
  --cp-login-toggle-knob: var(--cp-color-bg-container);
  --cp-login-toggle-shadow-color: color-mix(in srgb, var(--cp-color-primary-text) 20%, transparent);
  --cp-login-disabled-bg: var(--cp-color-bg-container-disabled);
  --cp-login-disabled-text-color: var(--cp-color-text-disabled);

  background: var(--cp-color-bg-layout);
  isolation: isolate;
}

:global(html[data-theme='dark'] .login-page) {
  --cp-login-panel-line-color: color-mix(in srgb, var(--cp-color-primary-border) 66%, transparent);
  --cp-login-logo-color: var(--cp-color-primary-text);
  --cp-login-title-color: var(--cp-color-text-heading);
  --cp-login-brand-title-color: var(--cp-color-white);
  --cp-login-brand-caption-color: var(--cp-color-text-tertiary);
  --cp-login-description-color: color-mix(in srgb, var(--cp-color-text-secondary) 82%, var(--cp-color-text));
  --cp-login-label-color: color-mix(in srgb, var(--cp-color-text) 86%, var(--cp-color-white));
  --cp-login-input-bg: color-mix(in srgb, var(--cp-color-bg-spotlight) 58%, transparent);
  --cp-login-input-hover-bg: color-mix(in srgb, var(--cp-color-fill-tertiary) 62%, transparent);
  --cp-login-input-active-bg: color-mix(in srgb, var(--cp-color-fill-tertiary) 72%, transparent);
  --cp-login-input-icon-color: var(--cp-color-primary-text);
  --cp-login-placeholder-color: var(--cp-color-text-quaternary);
  --cp-login-button-shadow-color: color-mix(in srgb, var(--cp-color-primary) 30%, transparent);
  --cp-login-toggle-bg: var(--cp-color-fill-tertiary);
  --cp-login-toggle-bg-hover: color-mix(in srgb, var(--cp-color-fill-tertiary) 94%, var(--cp-color-white));
  --cp-login-toggle-sun-color: var(--cp-color-text-tertiary);
  --cp-login-toggle-moon-color: var(--cp-color-white);
  --cp-login-toggle-knob: var(--cp-color-primary-hover);
  --cp-login-toggle-shadow-color: color-mix(in srgb, var(--cp-color-primary-text) 30%, transparent);
}
</style>
