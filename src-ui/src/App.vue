<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, ref, watch } from 'vue'
import { useDark } from '@vueuse/core'
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
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Badge } from '@/components/ui/badge'
import { AlertCircle, X } from 'lucide-vue-next'

useDark()

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

const DEFAULT_WIDTH = 720
const TAB_HEIGHTS: Record<Tab, number> = {
  session: 600,
  layout: 620,
  log: 780,
}
const ANIM_DURATION_MS = 220
let currentSize = { width: 0, height: 0 }
let animFrame: number | null = null

async function setWindowSize(w: number, h: number) {
  currentSize = { width: w, height: h }
  try {
    await getCurrentWindow().setSize(new LogicalSize(w, h))
  } catch (_e) {
    /* non-Tauri */
  }
}

async function getCurrentSize(): Promise<{ width: number; height: number }> {
  try {
    const size = await getCurrentWindow().innerSize()
    return { width: size.width, height: size.height }
  } catch {
    return { width: DEFAULT_WIDTH, height: TAB_HEIGHTS.session }
  }
}

function animateToHeight(targetH: number) {
  if (animFrame !== null) cancelAnimationFrame(animFrame)
  const startH = currentSize.height
  if (startH === targetH) return
  const t0 = performance.now()
  const tick = () => {
    const t = Math.min(1, (performance.now() - t0) / ANIM_DURATION_MS)
    const e = 1 - Math.pow(1 - t, 3)
    const h = Math.round(startH + (targetH - startH) * e)
    setWindowSize(currentSize.width, h)
    if (t < 1) {
      animFrame = requestAnimationFrame(tick)
    } else {
      animFrame = null
    }
  }
  animFrame = requestAnimationFrame(tick)
}

watch(tab, () => {
  const height = TAB_HEIGHTS[tab.value]
  animateToHeight(height)
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
  permsOk.value = await checkPermissions().catch(() => false)

  const size = await getCurrentSize()
  currentSize = size
  const height = TAB_HEIGHTS[tab.value]
  if (Math.abs(size.height - height) > 10) {
    await setWindowSize(size.width, height)
  }

  try {
    const win = getCurrentWindow()
    await win.onResized(async () => {
      const newSize = await getCurrentSize()
      currentSize = newSize
    })
  } catch {
    /* non-Tauri */
  }

  try {
    const r = await currentRole()
    applyRole(r)
  } catch (err) {
    console.error('Failed to get current role:', err)
  }

  unlistenRole = await listenRole((r) => {
    applyRole(r)
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
    try {
      const wasNew =
        (e.side === 'local' && layoutStore.local === null) ||
        (e.side === 'remote' && layoutStore.remote === null)
      layoutStore.setHost({
        side: e.side,
        instanceName: e.side === 'local' ? t('layout.thisHost') : link.peer || t('layout.remote'),
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
})

async function grantPermissions() {
  await requestPermissions().catch(() => {})
}

async function recheckPermissions() {
  permsChecking.value = true
  permsOk.value = await checkPermissions().catch(() => false)
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
  <main class="min-h-screen bg-background p-3">
    <div class="max-w-4xl mx-auto space-y-3">
      <div class="flex items-center justify-between px-1">
        <div class="flex items-center gap-2 text-lg font-semibold">
          <Badge :variant="linkStatusVariant" class="h-2 w-2 p-0" />
          MouseFly
        </div>
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

      <div class="space-y-3">
        <Alert v-if="!permsOk && !permsDismissed" variant="default" class="relative pr-8">
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
          <button
            class="absolute top-2 right-2 p-1 rounded-md hover:bg-muted transition-colors"
            @click="permsDismissed = true"
          >
            <X class="h-4 w-4" />
          </button>
        </Alert>

        <Tabs v-model="tab" class="w-full">
          <TabsList class="grid w-full grid-cols-3">
            <TabsTrigger value="session">{{ t('app.tabs.session') }}</TabsTrigger>
            <TabsTrigger value="layout">{{ t('app.tabs.layout') }}</TabsTrigger>
            <TabsTrigger value="log">{{ t('app.tabs.log') }}</TabsTrigger>
          </TabsList>

          <div class="mt-4">
            <div v-show="tab === 'session'">
              <SessionView />
            </div>
            <div v-show="tab === 'layout'">
              <LayoutView />
            </div>
            <div v-show="tab === 'log'">
              <LogView />
            </div>
          </div>
        </Tabs>
      </div>
    </div>
  </main>
</template>
