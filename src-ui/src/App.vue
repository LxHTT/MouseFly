<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, ref } from 'vue'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { useLinkStore } from './stores/link'
import { listenLinkHealth, listenLinkStatus, listenRole } from './ipc'
import LinkView from './views/LinkView.vue'
import PairingView from './views/PairingView.vue'

type Tab = 'link' | 'pair'
const tab = ref<Tab>('link')
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

const tabClass = (t: Tab) =>
  computed(() =>
    tab.value === t
      ? 'px-3 py-1.5 text-xs uppercase tracking-widest border-b-2 border-blue-500 text-zinc-100'
      : 'px-3 py-1.5 text-xs uppercase tracking-widest border-b-2 border-transparent text-zinc-500 hover:text-zinc-300',
  )
</script>

<template>
  <main class="min-h-screen bg-zinc-950 text-zinc-100 font-mono p-6">
    <div class="rounded-lg border border-zinc-800 bg-zinc-900/40 p-5 space-y-4">
      <header class="flex items-center justify-between">
        <h1 class="text-xl font-semibold tracking-tight">
          MouseFly
          <span class="ml-1 text-zinc-500 text-xs">phase 2</span>
        </h1>
        <nav class="flex gap-1 -mb-1">
          <button :class="tabClass('link').value" @click="tab = 'link'">Link</button>
          <button :class="tabClass('pair').value" @click="tab = 'pair'">Pair</button>
        </nav>
      </header>

      <LinkView v-if="tab === 'link'" />
      <PairingView v-else />
    </div>
  </main>
</template>
