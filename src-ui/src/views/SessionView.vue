<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { useLinkStore } from '../stores/link'
import { usePairingStore, type DiscoveredPeer } from '../stores/pairing'
import {
  cancelPairing,
  formatPeerAddr,
  getLocalIdentity,
  getPairingState,
  listenDiscoveredPeers,
  listenPairingCode,
  listenPairingLocked,
  listenPairingResult,
  listPairedPeers,
  pickPeerIp,
  startAdvertising,
  startLink,
  startPairInitiator,
  startPairResponder,
  stopAdvertising,
  stopLink,
  type LocalIdentity,
} from '../ipc'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Badge } from '@/components/ui/badge'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Checkbox } from '@/components/ui/checkbox'
import { Copy, CheckCircle2, XCircle } from 'lucide-vue-next'

const link = useLinkStore()
const pairing = usePairingStore()
const { t } = useI18n()
const identity = ref<LocalIdentity | null>(null)
const now = ref(Math.floor(Date.now() / 1000))
let tickHandle: ReturnType<typeof setInterval> | null = null
let unlistens: UnlistenFn[] = []

const useCustomCode = ref(false)
const customCode = ref('')
const customCodeError = ref('')
type Ttl = 'never' | 5 | 30 | 60
const ttlChoice = ref<Ttl>(5)
const ttlSeconds = computed(() =>
  ttlChoice.value === 'never' ? null : ttlChoice.value * 60,
)
const advancedOpen = ref(false)
const listenAddr = ref('0.0.0.0:7878')
const enableInject = ref(true)

const joinCode = ref('')
const manualPeer = ref('')

type Status =
  | { kind: 'idle' }
  | { kind: 'hosting'; code: string; expiresUnix: number }
  | { kind: 'joining'; peerLabel: string }
  | { kind: 'linked' }
  | { kind: 'failed'; reason: string }
const status = ref<Status>({ kind: 'idle' })

const hostingPeer = ref<DiscoveredPeer | null>(null)

const codeRemaining = computed(() => {
  if (status.value.kind !== 'hosting' || status.value.expiresUnix === 0) return null
  return Math.max(0, status.value.expiresUnix - now.value)
})
const codeRemainingLabel = computed(() => {
  const s = codeRemaining.value
  if (s === null) return 'no expiry'
  const m = Math.floor(s / 60)
  const r = s % 60
  return `${m}:${String(r).padStart(2, '0')}`
})
const codeProgress = computed(() => {
  if (status.value.kind !== 'hosting' || codeRemaining.value === null) return 1
  const totalSeconds = (typeof ttlChoice.value === 'number' ? ttlChoice.value : 5) * 60
  return Math.max(0, Math.min(1, codeRemaining.value / totalSeconds))
})

const visiblePeers = computed(() =>
  pairing.discovered.filter((p) => !p.is_self),
)

const formattedCode = (raw: string) =>
  raw.length === 6 && /^\d+$/.test(raw)
    ? `${raw.slice(0, 3)} ${raw.slice(3)}`
    : raw

const linkActive = computed(
  () => link.role === 'sender' || link.role === 'receiver',
)

const p50ms = computed(() => (link.p50us / 1000).toFixed(1))
const p99ms = computed(() => (link.p99us / 1000).toFixed(1))
const offsetMs = computed(() => (link.offsetNs / 1e6).toFixed(2))

watch(useCustomCode, (v) => {
  if (!v) customCode.value = ''
  customCodeError.value = ''
})

watch(() => link.role, async (role, oldRole) => {
  if (role === 'idle' && (status.value.kind === 'linked' || status.value.kind === 'hosting')) {
    status.value = { kind: 'idle' }
  } else if ((role === 'sender' || role === 'receiver') && oldRole === 'idle' && status.value.kind === 'idle') {
    const hasActiveConnection = link.p50us > 0 || link.eps > 0
    if (hasActiveConnection) {
      status.value = { kind: 'linked' }
    } else {
      const pairingState = await getPairingState().catch(() => null)
      if (pairingState) {
        status.value = {
          kind: 'hosting',
          code: pairingState.code,
          expiresUnix: pairingState.expires_unix,
        }
      }
    }
  }
})

watch(
  () => link.role,
  (role) => {
    if (role === 'idle' && status.value.kind === 'linked') {
      status.value = { kind: 'idle' }
    }
  },
)

function validateCustomCode(): boolean {
  const c = customCode.value.trim()
  if (c.length < 6) {
    customCodeError.value = t('session.setup.codeErrorLength')
    return false
  }
  if (!/^[A-Za-z0-9]+$/.test(c)) {
    customCodeError.value = t('session.setup.codeErrorChars')
    return false
  }
  customCodeError.value = ''
  return true
}

async function setupSession() {
  void ttlLabel.value
  if (useCustomCode.value && !validateCustomCode()) return
  customCodeError.value = ''
  try {
    await startLink({
      kind: 'receiver',
      listen: listenAddr.value,
      inject: enableInject.value,
    })
    await startAdvertising()
    await startPairResponder({
      code: useCustomCode.value ? customCode.value.trim() : null,
      ttl_seconds: ttlSeconds.value,
    })
  } catch (e) {
    stopAdvertising().catch(() => undefined)
    stopLink().catch(() => undefined)
    cancelPairing().catch(() => undefined)
    status.value = { kind: 'failed', reason: String(e) }
  }
}

async function stopSession() {
  await cancelPairing().catch(() => undefined)
  await stopAdvertising().catch(() => undefined)
  await stopLink().catch(() => undefined)
  status.value = { kind: 'idle' }
  hostingPeer.value = null
  joinCode.value = ''
  customCode.value = ''
  useCustomCode.value = false
}

function pickPeer(peer: DiscoveredPeer) {
  hostingPeer.value = peer
  joinCode.value = ''
  status.value = { kind: 'joining', peerLabel: peer.instance_name }
}

const ttlLabel = computed(() => {
  if (typeof ttlChoice.value !== 'number') return t('session.setup.ttlNever')
  const minutes = ttlChoice.value
  return t('session.setup.ttlEvery', {
    value: minutes >= 60 ? '1 h' : `${minutes} min`,
  })
})

function pickManual() {
  if (!manualPeer.value.trim()) return
  hostingPeer.value = null
  joinCode.value = ''
  status.value = { kind: 'joining', peerLabel: manualPeer.value.trim() }
}

async function submitJoin() {
  let pairAddr: string | null = null
  let dataAddr: string | null = null
  if (hostingPeer.value) {
    const ip = pickPeerIp(hostingPeer.value.addrs)
    if (!ip) {
      status.value = { kind: 'failed', reason: t('session.failed.noUsableAddress') }
      return
    }
    pairAddr = formatPeerAddr(ip, hostingPeer.value.port)
    dataAddr = formatPeerAddr(
      ip,
      hostingPeer.value.data_port || 7878,
    )
  } else {
    const m = manualPeer.value.trim()
    pairAddr = m
    dataAddr = m
  }
  if (!pairAddr) return

  try {
    await startPairInitiator(pairAddr, joinCode.value.replace(/\s+/g, ''))
  } catch (e) {
    status.value = { kind: 'failed', reason: String(e) }
  }
  pendingDataAddr.value = dataAddr
}

const pendingDataAddr = ref<string | null>(null)

async function copy(text: string) {
  try {
    await navigator.clipboard.writeText(text)
  } catch {
    /* ignore */
  }
}

onMounted(async () => {
  identity.value = await getLocalIdentity().catch(() => null)
  pairing.paired = await listPairedPeers().catch(() => [])

  if (link.role === 'sender' || link.role === 'receiver') {
    const hasActiveConnection = link.p50us > 0 || link.eps > 0
    if (hasActiveConnection) {
      status.value = { kind: 'linked' }
    } else {
      const pairingState = await getPairingState().catch(() => null)
      if (pairingState) {
        status.value = {
          kind: 'hosting',
          code: pairingState.code,
          expiresUnix: pairingState.expires_unix,
        }
      }
    }
  }

  unlistens.push(
    await listenDiscoveredPeers((peers) => {
      pairing.discovered = peers
    }),
    await listenPairingCode((e) => {
      status.value = {
        kind: 'hosting',
        code: e.code,
        expiresUnix: e.expires_unix,
      }
    }),
    await listenPairingLocked((e) => {
      status.value = { kind: 'failed', reason: e.reason }
    }),
    await listenPairingResult((e) => {
      if (!e.ok) {
        status.value = {
          kind: 'failed',
          reason: e.reason ?? 'pairing failed',
        }
        return
      }
      if (pendingDataAddr.value) {
        const target = pendingDataAddr.value
        pendingDataAddr.value = null
        startLink({ kind: 'sender', peer: target })
          .then(() => {
            status.value = { kind: 'linked' }
            listPairedPeers().then((p) => (pairing.paired = p))
          })
          .catch((err) => {
            status.value = { kind: 'failed', reason: String(err) }
          })
      } else {
        status.value = { kind: 'linked' }
        listPairedPeers().then((p) => (pairing.paired = p))
      }
    }),
  )

  tickHandle = setInterval(() => {
    now.value = Math.floor(Date.now() / 1000)
  }, 1000)
})

onBeforeUnmount(() => {
  unlistens.forEach((u) => u())
  unlistens = []
  if (tickHandle !== null) clearInterval(tickHandle)
})
</script>

<template>
  <div class="space-y-3">
    <!-- Active linked session -->
    <div v-if="linkActive && status.kind === 'linked'" class="space-y-3">
      <Card>
        <CardHeader class="pb-2">
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-2">
              <CheckCircle2 class="h-3.5 w-3.5 text-green-500" />
              <CardTitle class="text-sm">
                {{
                  link.role === 'sender'
                    ? t('session.success.titleSender')
                    : t('session.success.titleReceiver')
                }}
              </CardTitle>
            </div>
            <Button size="sm" variant="outline" class="h-7 text-xs" @click="stopSession">
              {{
                link.role === 'sender'
                  ? t('session.success.leave')
                  : t('session.success.stop')
              }}
            </Button>
          </div>
        </CardHeader>
        <CardContent class="space-y-2">
          <div class="flex items-center gap-2 text-xs">
            <span class="text-muted-foreground">{{ t('session.metrics.peer') }}</span>
            <code class="text-[10px] bg-muted px-1 py-0.5 rounded">{{ link.peer || '—' }}</code>
          </div>
          <div class="grid grid-cols-4 gap-2 text-center">
            <div class="space-y-0.5">
              <div class="text-[10px] text-muted-foreground">{{ t('session.metrics.latencyP50') }}</div>
              <div class="text-base font-semibold tabular-nums">{{ p50ms }}</div>
            </div>
            <div class="space-y-0.5">
              <div class="text-[10px] text-muted-foreground">{{ t('session.metrics.latencyP99') }}</div>
              <div class="text-base font-semibold tabular-nums">{{ p99ms }}</div>
            </div>
            <div class="space-y-0.5">
              <div class="text-[10px] text-muted-foreground">{{ t('session.metrics.eventsPerSec') }}</div>
              <div class="text-base font-semibold tabular-nums">{{ link.eps }}</div>
            </div>
            <div class="space-y-0.5">
              <div class="text-[10px] text-muted-foreground">{{ t('session.metrics.clockOffset') }}</div>
              <div class="text-base font-semibold tabular-nums">{{ offsetMs }}</div>
            </div>
          </div>
        </CardContent>
      </Card>
      <p class="text-[10px] text-muted-foreground px-1">
        {{ t('session.metrics.killSwitch') }}
        <kbd class="px-1 bg-muted rounded text-[9px]">Ctrl+⌘+⇧+Esc</kbd> (mac) /
        <kbd class="px-1 bg-muted rounded text-[9px]">Ctrl+Win+⇧+Esc</kbd> (Win)
      </p>
    </div>

    <!-- Hosting -->
    <Card v-else-if="status.kind === 'hosting'">
      <CardHeader class="pb-2">
        <CardTitle class="text-sm">{{ t('session.hosting.title') }}</CardTitle>
      </CardHeader>
      <CardContent class="space-y-2">
        <div class="border rounded-lg overflow-hidden">
          <div class="text-2xl font-light tabular-nums tracking-[0.2em] text-center py-4">
            {{ formattedCode(status.code) }}
          </div>
          <div v-if="codeRemaining !== null" class="h-1 bg-muted">
            <div
              class="h-full bg-primary transition-[width] duration-1000 ease-linear"
              :style="{ width: codeProgress * 100 + '%' }"
            />
          </div>
        </div>
        <p class="text-[10px] text-muted-foreground">
          {{ t('session.hosting.hint') }}
          <span v-if="codeRemaining !== null">
            {{ t('session.hosting.refreshIn') }}
            <span class="font-mono">{{ codeRemainingLabel }}</span>.
          </span>
          <span v-else>{{ t('session.hosting.neverExpires') }}</span>
        </p>
        <Button size="sm" variant="outline" class="w-full h-7 text-xs" @click="stopSession">
          {{ t('session.hosting.stop') }}
        </Button>
      </CardContent>
    </Card>

    <!-- Joining -->
    <Card v-else-if="status.kind === 'joining'">
      <CardHeader class="pb-2">
        <CardTitle class="text-sm">{{ t('session.joining.title', { peer: status.peerLabel }) }}</CardTitle>
      </CardHeader>
      <CardContent class="space-y-2">
        <Input
          v-model="joinCode"
          class="text-lg tabular-nums tracking-widest text-center h-12"
          :placeholder="t('session.joining.placeholder')"
          autofocus
        />
        <div class="flex gap-2">
          <Button
            class="flex-1 h-7 text-xs"
            size="sm"
            :disabled="joinCode.replace(/\s+/g, '').length < 6"
            @click="submitJoin"
          >
            {{ t('session.joining.submit') }}
          </Button>
          <Button size="sm" variant="outline" class="h-7 text-xs" @click="status = { kind: 'idle' }">
            {{ t('session.joining.cancel') }}
          </Button>
        </div>
      </CardContent>
    </Card>

    <!-- Failed -->
    <Alert v-else-if="status.kind === 'failed'" variant="destructive">
      <XCircle class="h-4 w-4" />
      <AlertTitle>{{ t('session.failed.title') }}</AlertTitle>
      <AlertDescription class="space-y-3">
        <p>{{ status.reason }}</p>
        <Button size="sm" variant="outline" @click="status = { kind: 'idle' }">
          {{ t('session.failed.tryAgain') }}
        </Button>
      </AlertDescription>
    </Alert>

    <!-- Idle: horizontal two-column layout -->
    <div v-else class="grid grid-cols-2 gap-3">
      <!-- Left column: Setup & Join -->
      <div class="space-y-3">
        <!-- Setup form -->
        <Card>
          <CardHeader class="pb-2">
            <CardTitle class="text-sm">{{ t('session.setup.title') }}</CardTitle>
          </CardHeader>
          <CardContent class="space-y-2">
            <div class="flex items-center space-x-2">
              <Checkbox
                id="custom-code"
                v-model:checked="useCustomCode"
              />
              <Label for="custom-code" class="cursor-pointer text-xs">{{ t('session.setup.customCode') }}</Label>
            </div>
            <div v-if="useCustomCode" class="space-y-1">
              <Input
                v-model="customCode"
                :placeholder="t('session.setup.codePlaceholder')"
                class="h-8 text-xs"
                @blur="validateCustomCode"
              />
              <p v-if="customCodeError" class="text-[10px] text-destructive">
                {{ customCodeError }}
              </p>
            </div>

            <div class="flex items-center gap-2">
              <Label class="text-[10px]">{{ t('session.setup.refresh') }}</Label>
              <Select v-model="ttlChoice" :disabled="useCustomCode">
                <SelectTrigger class="w-[140px] h-7 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="5">{{ t('session.setup.ttlEvery', { value: '5 min' }) }}</SelectItem>
                  <SelectItem value="30">{{ t('session.setup.ttlEvery', { value: '30 min' }) }}</SelectItem>
                  <SelectItem value="60">{{ t('session.setup.ttlEvery', { value: '1 h' }) }}</SelectItem>
                  <SelectItem value="never">{{ t('session.setup.ttlNever') }}</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <details
              class="border rounded-lg"
              :open="advancedOpen"
              @toggle="advancedOpen = ($event.target as HTMLDetailsElement).open"
            >
              <summary class="px-2 py-1.5 cursor-pointer hover:bg-muted/50 text-xs">
                {{ t('session.setup.advanced') }}
              </summary>
              <div class="px-2 pb-2 space-y-2">
                <div class="space-y-1">
                  <Label class="text-[10px]">{{ t('session.setup.listenAddr') }}</Label>
                  <Input v-model="listenAddr" placeholder="0.0.0.0:7878" class="h-7 text-xs" />
                </div>
                <div class="flex items-center space-x-2">
                  <Checkbox
                    id="inject"
                    v-model:checked="enableInject"
                  />
                  <Label for="inject" class="cursor-pointer text-xs">{{ t('session.setup.injectEvents') }}</Label>
                </div>
              </div>
            </details>

            <Button class="w-full h-7 text-xs" size="sm" @click="setupSession">
              {{ t('session.setup.start') }}
            </Button>
          </CardContent>
        </Card>

        <!-- Discovered sessions -->
        <Card>
          <CardHeader class="pb-2">
            <div class="flex items-center justify-between">
              <CardTitle class="text-sm">{{ t('session.discover.title') }}</CardTitle>
              <Badge v-if="visiblePeers.length" variant="secondary" class="text-[10px] h-4">
                {{ visiblePeers.length }}
              </Badge>
            </div>
          </CardHeader>
          <CardContent class="space-y-2">
            <div v-if="visiblePeers.length" class="space-y-1.5">
              <div
                v-for="peer in visiblePeers"
                :key="peer.instance_name"
                class="flex items-center justify-between p-1.5 rounded-lg border hover:bg-muted/50 transition-colors"
              >
                <div class="min-w-0 flex-1">
                  <div class="text-xs font-medium">{{ peer.instance_name }}</div>
                  <div class="text-[10px] text-muted-foreground font-mono truncate">
                    {{
                      formatPeerAddr(
                        pickPeerIp(peer.addrs) ?? peer.addrs[0] ?? '?',
                        peer.data_port || peer.port,
                      )
                    }}
                  </div>
                </div>
                <Button size="sm" class="h-6 text-[10px]" @click="pickPeer(peer)">
                  {{ t('session.discover.join') }}
                </Button>
              </div>
            </div>
            <p v-else class="text-[10px] text-muted-foreground text-center py-2">
              {{ t('session.discover.scanning') }}
            </p>
            <details class="border rounded-lg">
              <summary class="px-2 py-1.5 cursor-pointer hover:bg-muted/50 text-xs">
                {{ t('session.discover.manualToggle') }}
              </summary>
              <div class="px-2 pb-2 pt-1 space-y-1.5">
                <Input
                  v-model="manualPeer"
                  :placeholder="t('session.discover.manualPlaceholder')"
                  class="text-xs h-7"
                />
                <Button
                  size="sm"
                  class="w-full h-6 text-[10px]"
                  :disabled="!manualPeer.trim()"
                  @click="pickManual"
                >
                  {{ t('session.discover.continue') }}
                </Button>
              </div>
            </details>
          </CardContent>
        </Card>
      </div>

      <!-- Right column: Identity & Paired peers -->
      <div class="space-y-3">
        <Card v-if="identity">
          <CardHeader class="pb-2">
            <CardTitle class="text-sm">{{ t('identity.label') }}</CardTitle>
          </CardHeader>
          <CardContent class="space-y-2">
            <div class="text-xs">{{ identity.instance_name }}</div>
            <div class="flex items-center gap-2">
              <code class="text-[10px] text-muted-foreground flex-1 truncate">{{ identity.host_id_hex }}</code>
              <Button size="sm" variant="ghost" class="h-6 w-6 p-0" @click="copy(identity.host_id_hex)">
                <Copy class="h-3 w-3" />
              </Button>
            </div>
          </CardContent>
        </Card>

        <Card v-if="pairing.paired.length">
          <CardHeader class="pb-2">
            <CardTitle class="text-sm">{{ t('session.paired.title') }}</CardTitle>
          </CardHeader>
          <CardContent class="space-y-1">
            <div
              v-for="p in pairing.paired"
              :key="p.host_id_hex"
              class="flex items-center justify-between text-xs py-1"
            >
              <span class="truncate">{{ p.instance_name }}</span>
              <code class="text-[10px] text-muted-foreground">{{ p.host_id_hex.slice(0, 8) }}…</code>
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  </div>
</template>
