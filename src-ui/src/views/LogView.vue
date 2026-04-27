<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useLogStore } from '../stores/log'

const { t } = useI18n()
const logStore = useLogStore()
const listEl = ref<HTMLElement | null>(null)
const autoScroll = ref(true)

watch(
  () => logStore.entries.length,
  async () => {
    if (!autoScroll.value) return
    await nextTick()
    if (listEl.value) listEl.value.scrollTop = listEl.value.scrollHeight
  },
)

function onScroll() {
  if (!listEl.value) return
  const { scrollTop, scrollHeight, clientHeight } = listEl.value
  autoScroll.value = scrollHeight - scrollTop - clientHeight < 40
}

const levelColor: Record<string, string> = {
  error: 'text-red-400',
  warn: 'text-amber-400',
  info: 'text-zinc-300',
  debug: 'text-zinc-500',
  trace: 'text-zinc-600',
}
</script>

<template>
  <section class="space-y-3">
    <div class="flex items-center justify-between">
      <h2 class="text-sm uppercase tracking-widest text-zinc-400">
        {{ t('app.tabs.log') }}
      </h2>
      <button
        class="text-xs px-2 py-1 rounded border border-zinc-700 hover:bg-zinc-800"
        @click="logStore.clear()"
      >
        {{ t('log.clear') }}
      </button>
    </div>
    <div
      ref="listEl"
      class="h-[480px] overflow-y-auto border border-zinc-800 rounded bg-zinc-950 p-2 space-y-0.5"
      @scroll="onScroll"
    >
      <div
        v-for="(entry, i) in logStore.entries"
        :key="i"
        class="text-[11px] leading-relaxed font-mono flex gap-2"
      >
        <span class="shrink-0 w-10 text-right text-zinc-600">
          {{ new Date(entry.ts).toLocaleTimeString([], { hour12: false }) }}
        </span>
        <span :class="levelColor[entry.level] ?? 'text-zinc-400'" class="shrink-0 w-10 uppercase">
          {{ entry.level }}
        </span>
        <span class="text-zinc-300 break-all">{{ entry.message }}</span>
      </div>
      <p v-if="!logStore.entries.length" class="text-xs text-zinc-600 text-center py-8">
        {{ t('log.empty') }}
      </p>
    </div>
  </section>
</template>
