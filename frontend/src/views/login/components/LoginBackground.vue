<template>
  <div class="pointer-events-none absolute inset-0 -z-1 overflow-hidden bg-cp-bg-layout" aria-hidden="true">
    <div class="absolute inset-y-0 left-0 hidden w-[47%] border-r border-cp-border-secondary bg-(--cp-layout-sider-bg) min-[980px]:block">
      <div class="absolute top-[28%] left-[13%] right-[12%]">
        <span class="text-xs tracking-[0.22em] text-cp-primary-text">NEW CPR / CONSOLE</span>
        <p class="my-6 font-display text-[clamp(48px,6vw,88px)] leading-none tracking-tight text-cp-text">
          New CPR<span class="text-cp-primary">.</span>
        </p>
        <div class="mt-12 grid grid-cols-3 gap-3">
          <div class="h-28 rounded-cp-card border border-cp-border-secondary bg-(--cp-card-bg)" />
          <div class="mt-6 h-28 rounded-cp-card bg-cp-primary-container" />
          <div class="mt-12 h-28 rounded-cp-card border border-cp-primary-border bg-(--cp-card-bg)" />
        </div>
        <div class="mt-8 h-px w-16 bg-cp-primary" />
      </div>
    </div>
  </div>
</template>
