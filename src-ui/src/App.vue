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
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Badge } from '@/components/ui/badge'
import { AlertCircle, X } from 'lucide-vue-next'

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

const TAB_SIZES: Record<Tab, { width: number; minHeight: number }> = {
  session: { width: 600, minHeight: 820 },
  layout: { width: 820, minHeight: 720 },
  log: { width: 600, minHeight: 720 },
}
const OUTER_PADDING = 40
const ANIM_DURATION_MS = 220
const cardRef = ref<HTMLElement | null>(null)
let resizeObserver: ResizeObserver | null = null
let currentSize = { width: 0, height: 0 }
let firstResize = true
let animFrame: number | null = null
let pendingTimer: ReturnType<typeof setTimeout> | null = null
let autoResizeEnabled = true

async function setWindow(w: number, h: number) {
  currentSize = { width: w, height: h }
  try {
    await getCurrentWindow().setSize(new LogicalSize(w, h))
  } catch (_e) {
    /* non-Tauri */
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
    const e = 1 - Math.pow(1 - t, 3)
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
  permsOk.value = await checkPermissions().catch(() => true)

  if (cardRef.value && typeof ResizeObserver !== 'undefined') {
    resizeObserver = new ResizeObserver(() => fitWindowToContent())
    resizeObserver.observe(cardRef.value)
  }
  fitWindowToContent()
  try {
    const r = await currentRole()
    applyRole(r)
  } catch {
    /* idle default */
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

async function grantPermissions() {
  await requestPermissions().catch(() => {})
}

async function recheckPermissions() {
  permsChecking.value = true
  permsOk.value = await checkPermissions().catch(() => true)
  permsChecking.value = false
}

const linkStatusVariant = computed(() => {
  if (link.statusSeverity === 'error') return 'destructive'
  if (link.statusSeverity === 'warn') return 'secondary'
  if (link.role === 'sender' || link.role === 'receiver') return 'default'
  return 'outline'
})
</script>

<template>
  <main class="min-h-screen bg-background p-5">
    <Card ref="cardRef" class="max-w-4xl mx-auto">
      <CardHeader>
        <div class="flex items-center justify-between">
          <CardTitle class="flex items-center gap-2">
            <Badge :variant="linkStatusVariant" class="h-2 w-2 p-0" />
            MouseFly
          </CardTitle>
          <Select v-model="currentLocale">
            <SelectTrigger class="w-[100px] h-8 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem v-for="l in LOCALES" :key="l.code" :value="l.code">
                {{ l.label }}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
      </CardHeader>

      <CardContent class="space-y-4">
        <Alert v-if="!permsOk && !permsDismissed" variant="default" class="relative">
          <AlertCircle class="h-4 w-4" />
          <AlertTitle>{{ t('app.permissions.title') }}</AlertTitle>
          <AlertDescription class="space-y-2">
            <p class="text-sm">{{ t('app.permissions.description') }}</p>
            <p class="text-xs text-muted-foreground">{{ t('app.permissions.steps') }}</p>
            <div class="flex gap-2 mt-3">
              <Button size="sm" @click="grantPermissions">
                {{ t('app.permissions.grant') }}
              </Button>
              <Button size="sm" variant="outline" :disabled="permsChecking" @click="recheckPermissions">
                {{ t('app.permissions.recheck') }}
              </Button>
            </div>
          </AlertDescription>
          <Button
            variant="ghost"
            size="icon"
            class="absolute top-2 right-2 h-6 w-6"
            @click="permsDismissed = true"
          >
            <X class="h-4 w-4" />
          </Button>
        </Alert>

        <Tabs v-model="tab" class="w-full">
          <TabsList class="grid w-full grid-cols-3">
            <TabsTrigger value="session">{{ t('app.tabs.session') }}</TabsTrigger>
            <TabsTrigger value="layout">{{ t('app.tabs.layout') }}</TabsTrigger>
            <TabsTrigger value="log">{{ t('app.tabs.log') }}</TabsTrigger>
          </TabsList>

          <TabsContent value="session" class="mt-4">
            <SessionView />
          </TabsContent>

          <TabsContent value="layout" class="mt-4">
            <LayoutView />
          </TabsContent>

          <TabsContent value="log" class="mt-4">
            <LogView />
          </TabsContent>
        </Tabs>
      </CardContent>
    </Card>
  </main>
</template>
