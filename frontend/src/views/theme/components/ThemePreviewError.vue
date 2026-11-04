<script setup lang="ts">
defineProps<{ error?: Error }>()
</script>

<template>
  <div class="grid min-h-96 place-content-center text-cp-text-secondary" role="alert">
    预览加载失败
  </div>
</template>
