<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, ref, watch } from 'vue'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window'
import { useLinkStore } from './stores/link'
import { useLayoutStore, type CanvasMonitor } from './stores/layout'
import { useLogStore } from './stores/log'
import {
  checkPermissions,
  currentRole,
  listenLayout,
  listenLinkDropped,
  listenLinkHealth,
  listenLinkStatus,
  listenLogEntry,
  listenPeerAddr,
  listenRole,
  monitorIdToString,
  requestPermissions,
  type WireMonitor,
} from './ipc'
import { LOCALES, setLocale, type Locale } from './i18n'
import { useI18n } from 'vue-i18n'
import SessionView from './views/SessionView.vue'
import LayoutView from './views/LayoutView.vue'
import LogView from './views/LogView.vue'

type Tab = 'session' | 'layout' | 'log'
const tab = ref<Tab>('session')
const link = useLinkStore()
const layoutStore = useLayoutStore()
const logStore = useLogStore()
const { t, locale } = useI18n()
const currentLocale = computed({
  get: () => locale.value as Locale,
  set: (v: Locale) => setLocale(v),
})
const permsOk = ref(true)
const permsChecking = ref(false)
const permsDismissed = ref(false)
let unlistenRole: UnlistenFn | null = null
let unlistenHealth: UnlistenFn | null = null
let unlistenStatus: UnlistenFn | null = null
let unlistenLayout: UnlistenFn | null = null
let unlistenDropped: UnlistenFn | null = null
let unlistenPeerAddr: UnlistenFn | null = null
let unlistenLogEntry: UnlistenFn | null = null

function mapMonitor(m: WireMonitor): CanvasMonitor {
  const [w, h] = m.logical_size_px
  const [x, y] = m.position_in_local_vd
  const mm = m.physical_size_mm
  return {
    id: monitorIdToString(m.id),
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

// Per-tab width. Session tab auto-fits height via ResizeObserver; Layout
// tab sets height once on entry then allows manual resize.
const TAB_SIZES: Record<Tab, { width: number; minHeight: number }> = {
  session: { width: 560, minHeight: 820 },
  layout: { width: 780, minHeight: 720 },
  log: { width: 560, minHeight: 720 },
}
const OUTER_PADDING = 40 // main.p-5 × 2 sides
const ANIM_DURATION_MS = 220
const cardRef = ref<HTMLElement | null>(null)
let resizeObserver: ResizeObserver | null = null
let currentSize = { width: 0, height: 0 }
let firstResize = true
let animFrame: number | null = null
let pendingTimer: ReturnType<typeof setTimeout> | null = null
// When true, ResizeObserver drives the window height (session tab).
// When false, the window was sized once and the user can resize freely (layout tab).
let autoResizeEnabled = true

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
  if (!cardRef.value || !autoResizeEnabled) return
  const cfg = TAB_SIZES[tab.value]
  const w = cfg.width
  const h = Math.max(cfg.minHeight, cardRef.value.offsetHeight + OUTER_PADDING)
  if (pendingTimer !== null) clearTimeout(pendingTimer)
  pendingTimer = setTimeout(() => animateToSize(w, h), 16)
}

watch(tab, async () => {
  await new Promise((r) => requestAnimationFrame(r))
  if (tab.value === 'session') {
    autoResizeEnabled = true
    fitWindowToContent()
  } else {
    // Layout tab: resize once to the layout size, then stop auto-resizing.
    autoResizeEnabled = true
    fitWindowToContent()
    await new Promise((r) => setTimeout(r, ANIM_DURATION_MS + 50))
    autoResizeEnabled = false
  }
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
  // Permission preflight.
  permsOk.value = await checkPermissions().catch(() => true)

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
    try {
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
    } catch (err) {
      console.error('layout event handling failed', err, e)
    }
  })
  unlistenDropped = await listenLinkDropped(() => {
    link.role = 'idle'
    link.p50us = 0
    link.p99us = 0
    link.eps = 0
    link.offsetNs = 0
  })
  unlistenPeerAddr = await listenPeerAddr((addr) => {
    link.peer = addr
  })
  unlistenLogEntry = await listenLogEntry((e) => {
    logStore.push({ ts: Date.now(), level: e.level as any, message: e.message })
  })
})

onBeforeUnmount(() => {
  unlistenRole?.()
  unlistenHealth?.()
  unlistenStatus?.()
  unlistenLayout?.()
  unlistenDropped?.()
  unlistenPeerAddr?.()
  unlistenLogEntry?.()
  resizeObserver?.disconnect()
})

const tabClass = (t: Tab) =>
  computed(() =>
    tab.value === t
      ? 'px-3 py-1.5 text-xs uppercase tracking-widest border-b-2 border-blue-500 text-zinc-100 transition-colors'
      : 'px-3 py-1.5 text-xs uppercase tracking-widest border-b-2 border-transparent text-zinc-500 hover:text-zinc-300 transition-colors',
  )

async function grantPermissions() {
  await requestPermissions().catch(() => {})
}

async function recheckPermissions() {
  permsChecking.value = true
  permsOk.value = await checkPermissions().catch(() => true)
  permsChecking.value = false
}

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
      <header class="flex items-center justify-between gap-3">
        <h1 class="text-xl font-semibold tracking-tight flex items-center gap-2">
          <span
            class="inline-block w-2 h-2 rounded-full transition-colors"
            :class="linkDot"
          />
          MouseFly
        </h1>
        <div class="flex items-center gap-2 -mb-1">
          <select
            v-model="currentLocale"
            class="bg-zinc-900 border border-zinc-800 rounded text-[10px] py-0.5 px-1.5 text-zinc-400 hover:text-zinc-200 focus:border-zinc-700 outline-none"
            :title="'language'"
          >
            <option v-for="l in LOCALES" :key="l.code" :value="l.code">
              {{ l.label }}
            </option>
          </select>
          <nav class="flex gap-1">
            <button :class="tabClass('session').value" @click="tab = 'session'">
              {{ t('app.tabs.session') }}
            </button>
            <button :class="tabClass('layout').value" @click="tab = 'layout'">
              {{ t('app.tabs.layout') }}
            </button>
            <button :class="tabClass('log').value" @click="tab = 'log'">
              {{ t('app.tabs.log') }}
            </button>
          </nav>
        </div>
      </header>

      <div
        v-if="!permsOk && !permsDismissed"
        class="rounded border border-amber-700/60 bg-amber-900/20 p-4 space-y-2"
      >
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-2">
            <span class="inline-block w-2 h-2 rounded-full bg-amber-500" />
            <span class="text-sm text-amber-300 font-medium">{{ t('app.permissions.title') }}</span>
          </div>
          <button
            class="text-zinc-500 hover:text-zinc-300 text-sm leading-none px-1"
            @click="permsDismissed = true"
          >
            &times;
          </button>
        </div>
        <p class="text-xs text-zinc-400 leading-relaxed">
          {{ t('app.permissions.description') }}
        </p>
        <p class="text-xs text-zinc-500 leading-relaxed">
          {{ t('app.permissions.steps') }}
        </p>
        <div class="flex gap-2">
          <button
            class="text-xs px-3 py-1.5 rounded bg-amber-700/40 border border-amber-700 hover:bg-amber-700/60 text-amber-200 transition-colors"
            @click="grantPermissions"
          >
            {{ t('app.permissions.grant') }}
          </button>
          <button
            class="text-xs px-3 py-1.5 rounded border border-zinc-700 hover:bg-zinc-800 transition-colors"
            :disabled="permsChecking"
            @click="recheckPermissions"
          >
            {{ t('app.permissions.recheck') }}
          </button>
        </div>
      </div>

      <Transition
        :enter-active-class="'transition-opacity duration-150 ease-out'"
        :enter-from-class="'opacity-0'"
        :enter-to-class="'opacity-100'"
        :leave-active-class="'transition-opacity duration-100 ease-in'"
        :leave-from-class="'opacity-100'"
        :leave-to-class="'opacity-0'"
        mode="out-in"
      >
        <KeepAlive>
          <SessionView v-if="tab === 'session'" />
          <LayoutView v-else-if="tab === 'layout'" />
          <LogView v-else />
        </KeepAlive>
      </Transition>
    </div>
  </main>
</template>
