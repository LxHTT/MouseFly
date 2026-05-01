<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  hostBounds,
  hostOuterRect,
  useLayoutStore,
  type HostLayout,
  type HostSide,
} from '../stores/layout'
import { listenLayoutEditLock, notifyLayoutEditing } from '../ipc'
import type { UnlistenFn } from '@tauri-apps/api/event'

const layout = useLayoutStore()
const { t } = useI18n()

const isDark = ref(false)
const remoteIsEditing = ref(false)
let unlistenEditLock: UnlistenFn | null = null

function updateTheme() {
  isDark.value = document.documentElement.classList.contains('dark')
}

onMounted(async () => {
  updateTheme()
  const observer = new MutationObserver(updateTheme)
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ['class'],
  })

  unlistenEditLock = await listenLayoutEditLock((editing) => {
    remoteIsEditing.value = editing
  })

  onBeforeUnmount(() => {
    observer.disconnect()
    unlistenEditLock?.()
  })
})

const SNAP_DIST = 24
const HIGHLIGHT_DIST = 8
const ZOOM_MIN = 0.1
const ZOOM_MAX = 3.0
const PADDING = 200

const svgEl = ref<SVGSVGElement | null>(null)

const view = reactive({
  panX: 0,
  panY: 0,
  zoom: 1,
  vw: 800,
  vh: 480,
  fitX: 0,
  fitY: 0,
  fitW: 800,
  fitH: 480,
})

interface DragState {
  kind: 'pan' | 'host'
  side?: HostSide
  startClientX: number
  startClientY: number
  startOffsetX: number
  startOffsetY: number
  pointerId: number
}
const drag = ref<DragState | null>(null)

const hosts = computed(() => {
  const list: HostLayout[] = []
  if (layout.local) list.push(layout.local)
  if (layout.remote) list.push(layout.remote)
  return list
})

function recomputeFit() {
  const list = hosts.value
  if (!list.length) {
    view.fitX = 0
    view.fitY = 0
    view.fitW = 800
    view.fitH = 480
    return
  }
  let minX = Infinity
  let minY = Infinity
  let maxX = -Infinity
  let maxY = -Infinity
  for (const h of list) {
    const r = hostOuterRect(h)
    if (r.x < minX) minX = r.x
    if (r.y < minY) minY = r.y
    if (r.x + r.width > maxX) maxX = r.x + r.width
    if (r.y + r.height > maxY) maxY = r.y + r.height
  }
  view.fitX = minX - PADDING
  view.fitY = minY - PADDING
  view.fitW = maxX - minX + PADDING * 2
  view.fitH = maxY - minY + PADDING * 2
}

function measure() {
  const el = svgEl.value
  if (!el) return
  const rect = el.getBoundingClientRect()
  view.vw = Math.max(rect.width, 1)
  view.vh = Math.max(rect.height, 1)
}

let resizeObserver: ResizeObserver | null = null

// Whether the user has panned/zoomed manually since the last fit. Until
// they do, we keep auto-refitting on viewport / data changes so the content
// stays centred.
const userInteracted = ref(false)

onMounted(() => {
  measure()
  resetView()
  if (svgEl.value) {
    resizeObserver = new ResizeObserver(() => {
      measure()
      if (!userInteracted.value) resetView()
    })
    resizeObserver.observe(svgEl.value)
  }
  window.addEventListener('keydown', onKeyDown)
})

onBeforeUnmount(() => {
  resizeObserver?.disconnect()
  window.removeEventListener('keydown', onKeyDown)
})

// Refit (and reset view if untouched) whenever monitor data lands or the
// user resets offsets. Without this, the canvas mounted with empty data
// would never recenter when the first layout event arrived.
watch(
  () => [layout.local, layout.remote],
  () => {
    recomputeFit()
    if (!userInteracted.value) resetView()
  },
  { deep: true },
)

function resetView() {
  recomputeFit()
  const sx = view.vw / view.fitW
  const sy = view.vh / view.fitH
  view.zoom = Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, Math.min(sx, sy)))
  view.panX = view.vw / 2 - (view.fitX + view.fitW / 2) * view.zoom
  view.panY = view.vh / 2 - (view.fitY + view.fitH / 2) * view.zoom
}

function onWheel(e: WheelEvent) {
  e.preventDefault()
  const el = svgEl.value
  if (!el) return
  const rect = el.getBoundingClientRect()
  const sx = e.clientX - rect.left
  const sy = e.clientY - rect.top
  const factor = Math.exp(-e.deltaY * 0.0015)
  const next = Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, view.zoom * factor))
  const ratio = next / view.zoom
  view.panX = sx - (sx - view.panX) * ratio
  view.panY = sy - (sy - view.panY) * ratio
  view.zoom = next
  userInteracted.value = true
}

function onPointerDownBackground(e: PointerEvent) {
  if (e.button !== 0) return
  const target = e.target as Element
  if (target && target.closest('[data-host]')) return
  drag.value = {
    kind: 'pan',
    startClientX: e.clientX,
    startClientY: e.clientY,
    startOffsetX: view.panX,
    startOffsetY: view.panY,
    pointerId: e.pointerId,
  }
  ;(e.currentTarget as Element).setPointerCapture(e.pointerId)
  userInteracted.value = true
}

function onPointerDownHost(e: PointerEvent, side: HostSide) {
  if (e.button !== 0) return
  e.stopPropagation()
  const host = side === 'local' ? layout.local : layout.remote
  if (!host) return
  drag.value = {
    kind: 'host',
    side,
    startClientX: e.clientX,
    startClientY: e.clientY,
    startOffsetX: host.offsetX,
    startOffsetY: host.offsetY,
    pointerId: e.pointerId,
  }
  ;(e.currentTarget as Element).setPointerCapture(e.pointerId)
  // Notify peer that we're editing the layout
  notifyLayoutEditing(true).catch(() => {})
}

function onPointerMove(e: PointerEvent) {
  const d = drag.value
  if (!d) return
  if (d.kind === 'pan') {
    view.panX = d.startOffsetX + (e.clientX - d.startClientX)
    view.panY = d.startOffsetY + (e.clientY - d.startClientY)
    return
  }
  if (d.kind === 'host' && d.side) {
    const dx = (e.clientX - d.startClientX) / view.zoom
    const dy = (e.clientY - d.startClientY) / view.zoom
    let nx = d.startOffsetX + dx
    let ny = d.startOffsetY + dy
    const snapped = applySnap(d.side, nx, ny)
    nx = snapped.x
    ny = snapped.y
    layout.setHostOffset(d.side, nx, ny)
  }
}

function onPointerUp(e: PointerEvent) {
  const d = drag.value
  if (!d) return
  ;(e.currentTarget as Element).releasePointerCapture?.(d.pointerId)
  // Notify peer that we've stopped editing the layout
  if (d.kind === 'host') {
    notifyLayoutEditing(false).catch(() => {})
  }
  drag.value = null
  layout.pushOffsetsToRust()
}

function onContextMenu(e: MouseEvent) {
  e.preventDefault()
  layout.resetOffsets()
  userInteracted.value = false
}

function onKeyDown(e: KeyboardEvent) {
  if (e.key === 'r' || e.key === 'R') {
    const t = e.target as HTMLElement | null
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return
    layout.resetOffsets()
    userInteracted.value = false
  }
}

interface CandidateRect {
  side: HostSide
  x: number
  y: number
  width: number
  height: number
}

function otherHostsRect(side: HostSide): CandidateRect[] {
  const list: CandidateRect[] = []
  if (side !== 'local' && layout.local) {
    const r = hostOuterRect(layout.local)
    list.push({ side: 'local', ...r })
  }
  if (side !== 'remote' && layout.remote) {
    const r = hostOuterRect(layout.remote)
    list.push({ side: 'remote', ...r })
  }
  return list
}

// Snap dragged host edges flush against any other host's outer edge when within
// SNAP_DIST and the perpendicular extent overlaps. Pick the closest valid pair.
function applySnap(side: HostSide, nx: number, ny: number) {
  const host = side === 'local' ? layout.local : layout.remote
  if (!host) return { x: nx, y: ny }
  const b = hostBounds(host)
  const draggedX = nx + b.minX
  const draggedY = ny + b.minY
  const draggedW = b.width
  const draggedH = b.height
  const draggedRight = draggedX + draggedW
  const draggedBottom = draggedY + draggedH

  let bestDx: { delta: number; abs: number } | null = null
  let bestDy: { delta: number; abs: number } | null = null

  for (const other of otherHostsRect(side)) {
    const overlapY = draggedY < other.y + other.height && draggedY + draggedH > other.y
    if (overlapY) {
      const candidates = [
        { delta: other.x - draggedRight },
        { delta: other.x + other.width - draggedX },
        { delta: other.x - draggedX },
        { delta: other.x + other.width - draggedRight },
      ]
      for (const c of candidates) {
        const abs = Math.abs(c.delta)
        if (abs <= SNAP_DIST && (!bestDx || abs < bestDx.abs)) {
          bestDx = { delta: c.delta, abs }
        }
      }
    }
    const overlapX = draggedX < other.x + other.width && draggedX + draggedW > other.x
    if (overlapX) {
      const candidates = [
        { delta: other.y - draggedBottom },
        { delta: other.y + other.height - draggedY },
        { delta: other.y - draggedY },
        { delta: other.y + other.height - draggedBottom },
      ]
      for (const c of candidates) {
        const abs = Math.abs(c.delta)
        if (abs <= SNAP_DIST && (!bestDy || abs < bestDy.abs)) {
          bestDy = { delta: c.delta, abs }
        }
      }
    }
  }

  return {
    x: bestDx ? nx + bestDx.delta : nx,
    y: bestDy ? ny + bestDy.delta : ny,
  }
}

interface EdgeHighlight {
  x1: number
  y1: number
  x2: number
  y2: number
}

const highlights = computed<EdgeHighlight[]>(() => {
  if (!layout.local || !layout.remote) return []
  const a = hostOuterRect(layout.local)
  const b = hostOuterRect(layout.remote)
  const lines: EdgeHighlight[] = []

  // Vertical adjacency: a's right ~ b's left, etc.
  const verticalPairs: Array<[number, number]> = [
    [a.x + a.width, b.x],
    [a.x, b.x + b.width],
    [a.x, b.x],
    [a.x + a.width, b.x + b.width],
  ]
  for (const [ax, bx] of verticalPairs) {
    if (Math.abs(ax - bx) <= HIGHLIGHT_DIST) {
      const y1 = Math.max(a.y, b.y)
      const y2 = Math.min(a.y + a.height, b.y + b.height)
      if (y2 > y1) {
        const x = (ax + bx) / 2
        lines.push({ x1: x, y1, x2: x, y2 })
      }
    }
  }

  const horizontalPairs: Array<[number, number]> = [
    [a.y + a.height, b.y],
    [a.y, b.y + b.height],
    [a.y, b.y],
    [a.y + a.height, b.y + b.height],
  ]
  for (const [ay, by] of horizontalPairs) {
    if (Math.abs(ay - by) <= HIGHLIGHT_DIST) {
      const x1 = Math.max(a.x, b.x)
      const x2 = Math.min(a.x + a.width, b.x + b.width)
      if (x2 > x1) {
        const y = (ay + by) / 2
        lines.push({ x1, y1: y, x2, y2: y })
      }
    }
  }

  return lines
})

const transform = computed(
  () => `translate(${view.panX} ${view.panY}) scale(${view.zoom})`,
)

function hostFill(side: HostSide) {
  return side === 'local' ? 'rgba(59,130,246,0.06)' : 'rgba(16,185,129,0.06)'
}
function hostStroke(side: HostSide) {
  return side === 'local' ? 'rgb(96,165,250)' : 'rgb(52,211,153)'
}
function monitorFill(side: HostSide) {
  return side === 'local' ? 'rgba(59,130,246,0.18)' : 'rgba(16,185,129,0.18)'
}
function monitorStroke(side: HostSide) {
  return side === 'local' ? 'rgba(147,197,253,0.9)' : 'rgba(110,231,183,0.9)'
}
function monitorTextColor(side: HostSide) {
  // Bright colors that work in both light and dark themes
  return side === 'local' ? 'rgb(147,197,253)' : 'rgb(110,231,183)'
}

function hostRect(host: HostLayout) {
  return hostOuterRect(host)
}

defineExpose({ resetView })
</script>

<template>
  <div class="relative w-full h-full select-none">
    <svg
      ref="svgEl"
      class="w-full h-full bg-zinc-100 dark:bg-zinc-950 cursor-grab"
      :class="{ 'cursor-grabbing': drag !== null }"
      :viewBox="`0 0 ${view.vw} ${view.vh}`"
      preserveAspectRatio="xMidYMid meet"
      @wheel.prevent="onWheel"
      @pointerdown="onPointerDownBackground"
      @pointermove="onPointerMove"
      @pointerup="onPointerUp"
      @pointercancel="onPointerUp"
      @contextmenu="onContextMenu"
    >
      <defs>
        <pattern
          id="grid"
          width="80"
          height="80"
          patternUnits="userSpaceOnUse"
          :patternTransform="transform"
        >
          <path d="M 80 0 L 0 0 0 80" fill="none" stroke="hsl(var(--color-border))" stroke-width="1" />
        </pattern>
      </defs>
      <rect width="100%" height="100%" fill="url(#grid)" />

      <g :transform="transform">
        <g
          v-for="host in hosts"
          :key="host.side"
          :data-host="host.side"
          :transform="`translate(${host.offsetX} ${host.offsetY})`"
          @pointerdown="onPointerDownHost($event, host.side)"
        >
          <rect
            :x="hostRect(host).x - host.offsetX - 12"
            :y="hostRect(host).y - host.offsetY - 12"
            :width="hostRect(host).width + 24"
            :height="hostRect(host).height + 24"
            :fill="hostFill(host.side)"
            :stroke="hostStroke(host.side)"
            stroke-width="1.5"
            stroke-dasharray="6 6"
            rx="8"
          />
          <text
            :x="hostRect(host).x - host.offsetX - 12"
            :y="hostRect(host).y - host.offsetY - 20"
            :fill="hostStroke(host.side)"
            font-size="22"
            font-family="ui-monospace, SFMono-Regular, monospace"
          >
            {{ host.instanceName }}
          </text>

          <g v-for="m in host.monitors" :key="m.id">
            <rect
              :x="m.posX"
              :y="m.posY"
              :width="m.widthPx"
              :height="m.heightPx"
              :fill="monitorFill(host.side)"
              :stroke="monitorStroke(host.side)"
              stroke-width="2"
              rx="4"
            />
            <text
              :x="m.posX + 16"
              :y="m.posY + 36"
              :fill="monitorTextColor(host.side)"
              font-size="22"
              font-family="ui-monospace, SFMono-Regular, monospace"
            >
              {{ m.name }} {{ m.widthPx }}×{{ m.heightPx }} @{{ m.scale }}x
            </text>
            <g v-if="m.primary">
              <rect
                :x="m.posX + m.widthPx - 64"
                :y="m.posY + 16"
                width="48"
                height="22"
                rx="4"
                :fill="monitorTextColor(host.side)"
                fill-opacity="0.2"
                :stroke="monitorTextColor(host.side)"
                stroke-width="1"
              />
              <text
                :x="m.posX + m.widthPx - 40"
                :y="m.posY + 32"
                text-anchor="middle"
                :fill="monitorTextColor(host.side)"
                font-size="14"
                font-family="ui-monospace, SFMono-Regular, monospace"
              >
                {{ t('layout.primaryBadge') }}
              </text>
            </g>
          </g>
        </g>

        <line
          v-for="(h, i) in highlights"
          :key="i"
          :x1="h.x1"
          :y1="h.y1"
          :x2="h.x2"
          :y2="h.y2"
          stroke="hsl(var(--color-destructive))"
          stroke-width="6"
          stroke-linecap="round"
          opacity="0.85"
        />
      </g>
    </svg>

    <!-- Remote editing overlay -->
    <div
      v-if="remoteIsEditing"
      class="absolute inset-0 bg-black/50 backdrop-blur-sm flex items-center justify-center pointer-events-none"
    >
      <div class="bg-zinc-900 border border-zinc-700 rounded-lg px-6 py-4 text-center">
        <div class="text-lg font-semibold text-zinc-100 mb-1">
          {{ t('layout.remoteEditing') }}
        </div>
        <div class="text-sm text-zinc-400">
          {{ t('layout.remoteEditingDesc') }}
        </div>
      </div>
    </div>

    <div
      class="absolute top-2 right-2 flex gap-1 text-[10px] uppercase tracking-widest"
    >
      <button
        class="px-2 py-1 rounded border border-zinc-700 bg-zinc-800/90 text-zinc-200 hover:bg-zinc-700 transition-colors backdrop-blur-sm"
        @click="resetView"
      >
        {{ t('layout.fit') }}
      </button>
    </div>
  </div>
</template>
