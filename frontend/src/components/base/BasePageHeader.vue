<script setup lang="ts">
defineProps<{
  title: string
  description?: string
}>()
</script>

<template>
  <header
    class="flex shrink-0 flex-wrap items-center justify-between gap-x-4 gap-y-3 pb-5"
  >
    <div class="min-w-0 flex-1 basis-64">
      <h1 class="m-0 font-display text-[25px] leading-tight tracking-tight text-cp-text sm:text-[28px]">
        {{ title }}
      </h1>
      <p
        v-if="description || $slots.description"
        class="mt-1.5 mb-0 flex items-center gap-2 text-cp-sm leading-relaxed text-cp-text-secondary"
      >
        <slot name="description">
          {{ description }}
        </slot>
      </p>
    </div>
    <div
      v-if="$slots.actions"
      class="mt-0.5 flex max-w-full flex-wrap items-center gap-2"
    >
      <slot name="actions" />
    </div>
  </header>
</template>
