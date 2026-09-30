import type { RouteRecordRaw } from 'vue-router'

import SessionRecoveryView from '@/views/session-recovery/index.vue'

export const routes: RouteRecordRaw[] = [
  {
    path: '/login',
    name: 'login',
    component: () => import('@/views/login/index.vue'),
  },
  {
    path: '/session-recovery',
    name: 'session-recovery',
    component: SessionRecoveryView,
  },
  {
    path: '/',
    component: () => import('@/layout/index.vue'),
    children: [
      {
        path: '',
        name: 'dashboard',
        component: () => import('@/views/dashboard/index.vue'),
      },
      {
        path: 'accounts',
        name: 'accounts',
        component: () => import('@/views/accounts/index.vue'),
      },
      {
        path: 'relogin',
        name: 'relogin',
        component: () => import('@/views/relogin/index.vue'),
      },
      {
        path: 'proxies',
        name: 'proxies',
        component: () => import('@/views/proxies/index.vue'),
      },
      {
        path: 'ipv6-egress',
        name: 'ipv6-egress',
        component: () => import('@/views/ipv6-egress/index.vue'),
      },
      {
        path: 'account-groups',
        name: 'account-groups',
        component: () => import('@/views/account-groups/index.vue'),
      },
      {
        path: 'api-keys',
        name: 'api-keys',
        component: () => import('@/views/api-keys/index.vue'),
      },
      {
        path: 'usage',
        name: 'usage',
        component: () => import('@/views/usage/index.vue'),
      },
      {
        path: 'quality-ops',
        name: 'quality-ops',
        component: () => import('@/views/quality-ops/index.vue'),
      },
      {
        path: 'request-captures',
        name: 'request-captures',
        component: () => import('@/views/request-captures/index.vue'),
      },
      {
        path: 'theme',
        name: 'theme',
        component: () => import('@/views/theme/index.vue'),
      },
      {
        path: 'settings',
        name: 'settings',
        component: () => import('@/views/settings/index.vue'),
      },
      {
        path: 'settings/backup',
        name: 'settings-backup',
        component: () => import('@/views/settings/index.vue'),
      },
      {
        path: 'settings/cleanup',
        name: 'settings-cleanup',
        component: () => import('@/views/settings/index.vue'),
      },
    ],
  },
  {
    path: '/:pathMatch(.*)*',
    redirect: '/',
  },
]
