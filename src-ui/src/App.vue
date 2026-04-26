<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, ref, watch } from 'vue'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window'
import { useLinkStore } from './stores/link'
import { useLayoutStore, type CanvasMonitor } from './stores/layout'
import {
  currentRole,
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

// Per-tab width + content-driven height with a per-tab minimum, animated
// over ~220ms with cubic ease-out. The layout canvas needs horizontal room;
// link/pair pick a comfortable minimum so the inner cards don't crush.
const TAB_SIZES: Record<Tab, { width: number; minHeight: number }> = {
  link: { width: 520, minHeight: 660 },
  layout: { width: 780, minHeight: 700 },
  pair: { width: 560, minHeight: 720 },
}
const OUTER_PADDING = 40 // main.p-5 × 2 sides
const ANIM_DURATION_MS = 220
const cardRef = ref<HTMLElement | null>(null)
let resizeObserver: ResizeObserver | null = null
let currentSize = { width: 0, height: 0 }
let firstResize = true
let animFrame: number | null = null
let pendingTimer: ReturnType<typeof setTimeout> | null = null

async function setWindow(w: number, h: number) {
  currentSize = { width: w, height: h }
  try {
    await getCurrentWindow().setSize(new LogicalSize(w, h))
  } catch (_e) {
    /* non-Tauri (vite preview, tests) */
  }
}

function animateToSize(targetW: number, targetH: number) {
  if (firstResize) {
    firstResize = false
    setWindow(targetW, targetH)
    return
  }
  if (animFrame !== null) cancelAnimationFrame(animFrame)
  const startW = currentSize.width
  const startH = currentSize.height
  if (startW === targetW && startH === targetH) return
  const t0 = performance.now()
  const tick = () => {
    const t = Math.min(1, (performance.now() - t0) / ANIM_DURATION_MS)
    const e = 1 - Math.pow(1 - t, 3) // cubic ease-out
    const w = Math.round(startW + (targetW - startW) * e)
    const h = Math.round(startH + (targetH - startH) * e)
    setWindow(w, h)
    if (t < 1) {
      animFrame = requestAnimationFrame(tick)
    } else {
      animFrame = null
    }
  }
  animFrame = requestAnimationFrame(tick)
}

function fitWindowToContent() {
  if (!cardRef.value) return
  const cfg = TAB_SIZES[tab.value]
  const w = cfg.width
  const h = Math.max(cfg.minHeight, cardRef.value.offsetHeight + OUTER_PADDING)
  // Coalesce a flurry of ResizeObserver calls into one animation tick.
  if (pendingTimer !== null) clearTimeout(pendingTimer)
  pendingTimer = setTimeout(() => animateToSize(w, h), 16)
}

watch(tab, async () => {
  await new Promise((r) => requestAnimationFrame(r))
  fitWindowToContent()
})

function applyRole(r: { kind: string; peer?: string; listen?: string; inject?: boolean }) {
  link.role = r.kind as typeof link.role
  if (r.kind === 'sender') {
    link.peer = r.peer ?? ''
    link.inject = false
  } else if (r.kind === 'receiver') {
    link.peer = r.listen ?? ''
    link.inject = r.inject ?? false
  } else {
    link.peer = ''
    link.inject = false
  }
}

onMounted(async () => {
  // Watch the inner card's size and resize the OS window to match.
  if (cardRef.value && typeof ResizeObserver !== 'undefined') {
    resizeObserver = new ResizeObserver(() => fitWindowToContent())
    resizeObserver.observe(cardRef.value)
  }
  fitWindowToContent()
  // Fetch the authoritative role first — Rust may have emitted "role" before
  // the webview's listener was registered.
  try {
    const r = await currentRole()
    applyRole(r)
  } catch {
    /* idle default is fine */
  }
  unlistenRole = await listenRole((r) => applyRole(r))
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
  resizeObserver?.disconnect()
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
  <main class="bg-zinc-950 text-zinc-100 font-mono p-5">
    <div
      ref="cardRef"
      class="rounded-lg border border-zinc-800 bg-zinc-900/40 p-5 space-y-4"
    >
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
