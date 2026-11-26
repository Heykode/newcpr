<script setup lang="ts">
import type { ProfileOptions, ProfilePreview, ProfileSelection } from './profile'
import type { ProfileEditorProps } from '@/components/request-profile/useProfileEditor'
import BaseSkeleton from '@/components/base/BaseSkeleton.vue'
import ProfileEditorFrame from '@/components/request-profile/ProfileEditorFrame.vue'
import { useProfileEditor } from '@/components/request-profile/useProfileEditor'
import { needsProfileInput } from './profile'
import ProfileFields from './ProfileFields.vue'

const props = defineProps<ProfileEditorProps<ProfileSelection>>()
const model = defineModel<ProfileSelection | null | undefined>({ required: true })
const { options, preview, effective, source, frame, loading, previewing } = useProfileEditor<ProfileSelection, ProfileOptions, ProfilePreview>(
  'openai',
  model,
  props,
  needsProfileInput,
  selection => selection.mode !== 'custom' && selection.versionMode !== 'fixed',
)
</script>

<template>
  <ProfileEditorFrame v-model="source" v-bind="frame" :side-by-side="!allowInherit">
    <template #source-extra>
      <slot name="source-extra" />
    </template>
    <ProfileFields
      v-if="options"
      :model-value="effective ?? null"
      :options="options"
      :preview="preview"
      :previewing="previewing"
      :disabled="disabled || loading"
      :aria-busy="loading || undefined"
      @update:model-value="model = $event"
    />
    <div v-else-if="loading && !allowInherit" class="grid gap-4" aria-hidden="true">
      <div v-for="row in 2" :key="row" class="grid gap-4 sm:grid-cols-2">
        <div v-for="field in 2" :key="field" class="grid gap-2">
          <BaseSkeleton class="h-4 w-20" />
          <BaseSkeleton class="h-cp-control" />
        </div>
      </div>
    </div>
  </ProfileEditorFrame>
</template>
