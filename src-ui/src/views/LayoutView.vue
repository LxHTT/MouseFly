<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import MonitorCanvas from '../components/MonitorCanvas.vue'
import { useLayoutStore } from '../stores/layout'

const layout = useLayoutStore()
const { t } = useI18n()

const hasLocal = computed(() => layout.local !== null)
const hasRemote = computed(() => layout.remote !== null)

onMounted(() => {
  if (new URLSearchParams(window.location.search).has('mock')) {
    layout.__dev_mock__()
  }
  // Dev: paste in the browser console to inject mock data:
  //   window.__mfMock?.()
  ;(window as unknown as { __mfMock?: () => void }).__mfMock = () =>
    layout.__dev_mock__()
})

function reset() {
  layout.resetOffsets()
}
</script>

<template>
  <section class="space-y-3 flex flex-col" style="min-height: 480px">
    <header class="flex items-center justify-between">
      <h2 class="text-sm uppercase tracking-widest text-zinc-400">
        {{ t('layout.title') }}
      </h2>
      <button
        class="text-xs px-2 py-1 rounded border border-zinc-700 hover:bg-zinc-800 disabled:opacity-50"
        :disabled="!hasLocal && !hasRemote"
        @click="reset"
      >
        {{ t('layout.reset') }}
      </button>
    </header>

    <div
      class="flex-1 relative rounded border border-zinc-800 overflow-hidden"
      style="min-height: 420px"
    >
      <MonitorCanvas v-if="hasLocal || hasRemote" />
      <div
        v-else
        class="absolute inset-0 flex items-center justify-center text-xs text-zinc-500 px-6 text-center leading-relaxed"
      >
        {{ t('layout.empty') }}
      </div>
    </div>

    <footer
      v-if="hasLocal || hasRemote"
      class="flex items-center gap-4 text-[10px] uppercase tracking-widest text-zinc-500"
    >
      <span class="flex items-center gap-2">
        <span class="inline-block w-3 h-3 rounded-sm border border-blue-500 bg-blue-500/20"></span>
        {{ t('layout.legendLocal') }}
      </span>
      <span class="flex items-center gap-2">
        <span
          class="inline-block w-3 h-3 rounded-sm border border-emerald-500 bg-emerald-500/20"
        ></span>
        {{ t('layout.legendRemote') }}
      </span>
      <span v-if="!hasRemote" class="text-zinc-600">{{ t('layout.waitingRemote') }}</span>
    </footer>
  </section>
</template>
