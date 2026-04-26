<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount } from 'vue'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { useLinkStore } from './stores/link'
import { listenLinkHealth, listenLinkStatus, listenRole } from './ipc'

const link = useLinkStore()
let unlistenRole: UnlistenFn | null = null
let unlistenHealth: UnlistenFn | null = null
let unlistenStatus: UnlistenFn | null = null

onMounted(async () => {
  unlistenRole = await listenRole((r) => {
    link.role = r.kind
    if (r.kind === 'sender') {
      link.peer = r.peer
      link.inject = false
    } else {
      link.peer = r.listen
      link.inject = r.inject
    }
  })
  unlistenHealth = await listenLinkHealth((h) => {
    link.p50us = h.p50_us
    link.p99us = h.p99_us
    link.eps = h.events_per_sec
    link.offsetNs = h.clock_offset_ns
  })
  unlistenStatus = await listenLinkStatus((s) => {
    link.statusSeverity = s.severity
    link.statusText = s.text
  })
})

onBeforeUnmount(() => {
  unlistenRole?.()
  unlistenHealth?.()
  unlistenStatus?.()
})

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
  <main class="min-h-screen bg-zinc-950 text-zinc-100 font-mono p-6">
    <div class="rounded-lg border border-zinc-800 bg-zinc-900/40 p-5 space-y-4">
      <header class="flex items-center justify-between">
        <h1 class="text-xl font-semibold tracking-tight">
          MouseFly
          <span class="ml-1 text-zinc-500 text-xs">phase 0</span>
        </h1>
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

      <div :class="['rounded border px-3 py-2 text-xs leading-relaxed', statusClass]">
        {{ link.statusText }}
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

      <p class="text-xs text-zinc-600 leading-relaxed">
        Kill switch: <kbd class="px-1 bg-zinc-800 rounded text-zinc-300">Ctrl + ⌘ + ⇧ + Esc</kbd>
      </p>
    </div>
  </main>
</template>
