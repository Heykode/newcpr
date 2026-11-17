<script setup lang="ts">
import type { ComponentSize } from './controls/types'
import { provideComponentSize } from './controls/size'

const props = defineProps<{ size?: ComponentSize }>()
provideComponentSize(() => props.size, 'config')
</script>

<template>
  <slot />
</template>
