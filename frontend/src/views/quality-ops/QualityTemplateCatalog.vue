<script setup lang="ts">
import type { QualityRuleTemplate } from '@/api/modules/quality-ops'
import { Pencil, Plus, RefreshCw, Trash2 } from '@lucide/vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import { qualityScheduleSummary } from './schedule'

defineProps<{ templates: QualityRuleTemplate[], loading: boolean, busy: boolean, error: string }>()
defineEmits<{ create: [], edit: [template: QualityRuleTemplate], remove: [template: QualityRuleTemplate], refresh: [] }>()
</script>

<template>
  <section class="min-w-0" aria-label="规则模板列表">
    <div class="mb-4 flex flex-wrap items-center justify-between gap-3">
      <h2 class="text-base font-semibold">
        规则模板 <span class="ml-2 text-cp-sm font-normal text-cp-text-secondary">{{ templates.length }}</span>
      </h2>
      <div class="flex items-center gap-2">
        <BaseIconButton label="刷新规则模板" :disabled="loading || busy" @click="$emit('refresh')">
          <RefreshCw class="size-4" />
        </BaseIconButton>
        <BaseButton :disabled="busy" @click="$emit('create')">
          <Plus class="size-4" />新建规则模板
        </BaseButton>
      </div>
    </div>
    <p v-if="error" class="mb-3 text-cp-error" role="alert">
      {{ error }}
    </p>
    <div class="divide-y divide-cp-border border-y border-cp-border">
      <article v-for="template in templates" :key="template.id" class="flex min-w-0 items-center justify-between gap-3 py-4">
        <div class="min-w-0">
          <h3 class="break-words font-semibold [overflow-wrap:anywhere]">
            {{ template.name }}
          </h3>
          <p class="mt-1 break-all text-cp-sm text-cp-text-secondary">
            {{ template.config.detectionMode === 'state_probe' ? '状态探针' : '题目检测' }} · {{ template.config.model }}
          </p>
          <p class="mt-1 break-words text-xs text-cp-text-secondary">
            {{ template.config.enabled ? '定时开启' : '监测暂停' }} · {{ qualityScheduleSummary(template.config) }} · 版本 {{ template.revision }}
          </p>
        </div>
        <div class="flex shrink-0 gap-1">
          <BaseIconButton :label="`编辑模板：${template.name}`" :disabled="busy" @click="$emit('edit', template)">
            <Pencil class="size-4" />
          </BaseIconButton>
          <BaseIconButton :label="`删除模板：${template.name}`" :disabled="busy" @click="$emit('remove', template)">
            <Trash2 class="size-4" />
          </BaseIconButton>
        </div>
      </article>
      <p v-if="!templates.length && !error" class="py-12 text-center text-cp-text-secondary">
        {{ loading ? '加载中…' : '暂无规则模板' }}
      </p>
    </div>
  </section>
</template>
