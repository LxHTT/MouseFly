<script setup lang="ts">
import { computed } from 'vue'
import { useLinkStore } from '../stores/link'

const link = useLinkStore()

const p50ms = computed(() => (link.p50us / 1000).toFixed(1))
const p99ms = computed(() => (link.p99us / 1000).toFixed(1))
const offsetMs = computed(() => (link.offsetNs / 1e6).toFixed(2))
const roleClass = computed(() => {
  if (link.role === 'sender') return 'bg-blue-700/40 text-blue-200 border-blue-700'
  if (link.role === 'receiver') return 'bg-emerald-700/40 text-emerald-200 border-emerald-700'
  return 'bg-zinc-700/40 text-zinc-300 border-zinc-700'
})
const statusClass = computed(() => {
  if (link.statusSeverity === 'error') return 'bg-red-900/40 border-red-800 text-red-200'
  if (link.statusSeverity === 'warn') return 'bg-amber-900/40 border-amber-800 text-amber-200'
  return 'bg-zinc-900/60 border-zinc-800 text-zinc-300'
})
</script>

<template>
  <section class="space-y-4">
    <header class="flex items-center justify-between">
      <h2 class="text-sm uppercase tracking-widest text-zinc-400">Link</h2>
      <span
        :class="['px-2 py-1 rounded text-[10px] uppercase tracking-widest border', roleClass]"
      >
        {{ link.role }}
      </span>
    </header>

    <div class="text-zinc-400 text-sm space-y-1">
      <div>
        <span class="text-zinc-500">peer:</span>
        <span class="text-zinc-200 ml-2">{{ link.peer || '—' }}</span>
      </div>
      <div v-if="link.role === 'receiver'">
        <span class="text-zinc-500">inject:</span>
        <span :class="['ml-2', link.inject ? 'text-emerald-400' : 'text-zinc-400']">
          {{ link.inject ? 'on' : 'off (loopback safe)' }}
        </span>
      </div>
    </div>

    <div class="grid grid-cols-2 gap-3">
      <div class="border border-zinc-800 rounded p-3">
        <div class="text-[10px] text-zinc-500 uppercase tracking-widest">latency p50</div>
        <div class="text-2xl tabular-nums">
          {{ p50ms }} <span class="text-sm text-zinc-500">ms</span>
        </div>
      </div>
      <div class="border border-zinc-800 rounded p-3">
        <div class="text-[10px] text-zinc-500 uppercase tracking-widest">latency p99</div>
        <div class="text-2xl tabular-nums">
          {{ p99ms }} <span class="text-sm text-zinc-500">ms</span>
        </div>
      </div>
      <div class="border border-zinc-800 rounded p-3">
        <div class="text-[10px] text-zinc-500 uppercase tracking-widest">events/sec</div>
        <div class="text-2xl tabular-nums">{{ link.eps }}</div>
      </div>
      <div class="border border-zinc-800 rounded p-3">
        <div class="text-[10px] text-zinc-500 uppercase tracking-widest">clock offset</div>
        <div class="text-2xl tabular-nums">
          {{ offsetMs }} <span class="text-sm text-zinc-500">ms</span>
        </div>
      </div>
    </div>

    <div :class="['rounded border px-3 py-2 text-xs leading-relaxed', statusClass]">
      {{ link.statusText }}
    </div>

    <p class="text-xs text-zinc-600 leading-relaxed">
      Kill switch:
      <kbd class="px-1 bg-zinc-800 rounded text-zinc-300">Ctrl + ⌘ + ⇧ + Esc</kbd>
      (mac) /
      <kbd class="px-1 bg-zinc-800 rounded text-zinc-300">Ctrl + Win + ⇧ + Esc</kbd>
      (Windows)
    </p>
  </section>
</template>
