<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, ref, watch } from 'vue'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window'
import { useLinkStore } from './stores/link'
import { useLayoutStore, type CanvasMonitor } from './stores/layout'
import {
  listenLayout,
  listenLinkHealth,
  listenLinkStatus,
  listenRole,
  type WireMonitor,
} from './ipc'
import LinkView from './views/LinkView.vue'
import LayoutView from './views/LayoutView.vue'
import PairingView from './views/PairingView.vue'

type Tab = 'link' | 'layout' | 'pair'
const tab = ref<Tab>('link')
const link = useLinkStore()
const layoutStore = useLayoutStore()
let unlistenRole: UnlistenFn | null = null
let unlistenHealth: UnlistenFn | null = null
let unlistenStatus: UnlistenFn | null = null
let unlistenLayout: UnlistenFn | null = null

function mapMonitor(m: WireMonitor): CanvasMonitor {
  const [w, h] = m.logical_size_px
  const [x, y] = m.position_in_local_vd
  const mm = m.physical_size_mm
  return {
    id: m.id[0].toString(16),
    name: m.name,
    widthPx: w,
    heightPx: h,
    scale: m.scale_factor,
    mmW: mm ? mm[0] : null,
    mmH: mm ? mm[1] : null,
    posX: x,
    posY: y,
    primary: m.primary,
  }
}

const TAB_SIZES: Record<Tab, { width: number; height: number }> = {
  link: { width: 480, height: 480 },
  layout: { width: 760, height: 600 },
  pair: { width: 540, height: 680 },
}

async function applyTabSize(t: Tab) {
  try {
    const w = getCurrentWindow()
    await w.setSize(new LogicalSize(TAB_SIZES[t].width, TAB_SIZES[t].height))
  } catch (_e) {
    /* ignore — non-Tauri context (vite preview, tests) */
  }
}

watch(tab, (t) => {
  applyTabSize(t)
})

onMounted(async () => {
  applyTabSize(tab.value)
  unlistenRole = await listenRole((r) => {
    link.role = r.kind
    if (r.kind === 'sender') {
      link.peer = r.peer
      link.inject = false
    } else if (r.kind === 'receiver') {
      link.peer = r.listen
      link.inject = r.inject
    } else {
      link.peer = ''
      link.inject = false
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
  unlistenLayout = await listenLayout((e) => {
    const wasNew =
      (e.side === 'local' && layoutStore.local === null) ||
      (e.side === 'remote' && layoutStore.remote === null)
    layoutStore.setHost({
      side: e.side,
      instanceName: e.side === 'local' ? 'This host' : link.peer || 'Remote',
      offsetX: 0,
      offsetY: 0,
      monitors: e.monitors.map(mapMonitor),
    })
    if (wasNew) layoutStore.resetOffsets()
  })
})

onBeforeUnmount(() => {
  unlistenRole?.()
  unlistenHealth?.()
  unlistenStatus?.()
  unlistenLayout?.()
})

const tabClass = (t: Tab) =>
  computed(() =>
    tab.value === t
      ? 'px-3 py-1.5 text-xs uppercase tracking-widest border-b-2 border-blue-500 text-zinc-100 transition-colors'
      : 'px-3 py-1.5 text-xs uppercase tracking-widest border-b-2 border-transparent text-zinc-500 hover:text-zinc-300 transition-colors',
  )

const linkDot = computed(() => {
  if (link.statusSeverity === 'error') return 'bg-red-500'
  if (link.statusSeverity === 'warn') return 'bg-amber-500'
  if (link.role === 'sender' || link.role === 'receiver') return 'bg-emerald-500'
  return 'bg-zinc-600'
})
</script>

<template>
  <main class="min-h-screen bg-zinc-950 text-zinc-100 font-mono p-5">
    <div class="rounded-lg border border-zinc-800 bg-zinc-900/40 p-5 space-y-4">
      <header class="flex items-center justify-between">
        <h1 class="text-xl font-semibold tracking-tight flex items-center gap-2">
          <span
            class="inline-block w-2 h-2 rounded-full transition-colors"
            :class="linkDot"
          />
          MouseFly
        </h1>
        <nav class="flex gap-1 -mb-1">
          <button :class="tabClass('link').value" @click="tab = 'link'">Link</button>
          <button :class="tabClass('layout').value" @click="tab = 'layout'">Layout</button>
          <button :class="tabClass('pair').value" @click="tab = 'pair'">Pair</button>
        </nav>
      </header>

      <Transition
        :enter-active-class="'transition-opacity duration-150 ease-out'"
        :enter-from-class="'opacity-0'"
        :enter-to-class="'opacity-100'"
        :leave-active-class="'transition-opacity duration-100 ease-in'"
        :leave-from-class="'opacity-100'"
        :leave-to-class="'opacity-0'"
        mode="out-in"
      >
        <LinkView v-if="tab === 'link'" />
        <LayoutView v-else-if="tab === 'layout'" />
        <PairingView v-else />
      </Transition>
    </div>
  </main>
</template>
