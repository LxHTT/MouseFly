<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { useLinkStore } from '../stores/link'
import { usePairingStore, type DiscoveredPeer } from '../stores/pairing'
import {
  cancelPairing,
  formatPeerAddr,
  getLocalIdentity,
  listenDiscoveredPeers,
  listenPairingCode,
  listenPairingLocked,
  listenPairingResult,
  listPairedPeers,
  pickPeerIp,
  startLink,
  startPairInitiator,
  startPairResponder,
  stopLink,
  type LocalIdentity,
} from '../ipc'

const link = useLinkStore()
const pairing = usePairingStore()
const identity = ref<LocalIdentity | null>(null)
const now = ref(Math.floor(Date.now() / 1000))
let tickHandle: ReturnType<typeof setInterval> | null = null
let unlistens: UnlistenFn[] = []

// --- Setup-session form -------------------------------------------------
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
const enableInject = ref(true) // session host wants the cursor to actually move

// --- Join-session form --------------------------------------------------
const joinCode = ref('')
const manualPeer = ref('')

// --- Server-side state mirror ------------------------------------------
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

function validateCustomCode(): boolean {
  const c = customCode.value.trim()
  if (c.length < 6) {
    customCodeError.value = 'at least 6 characters'
    return false
  }
  if (!/^[A-Za-z0-9]+$/.test(c)) {
    customCodeError.value = 'letters and digits only'
    return false
  }
  customCodeError.value = ''
  return true
}

async function setupSession() {
  if (useCustomCode.value && !validateCustomCode()) return
  customCodeError.value = ''
  try {
    await startPairResponder({
      code: useCustomCode.value ? customCode.value.trim() : null,
      ttl_seconds: ttlSeconds.value,
    })
    await startLink({
      kind: 'receiver',
      listen: listenAddr.value,
      inject: enableInject.value,
    })
    // The actual code arrives via the pairing-code event below.
  } catch (e) {
    status.value = { kind: 'failed', reason: String(e) }
  }
}

async function stopSession() {
  await cancelPairing().catch(() => undefined)
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
      status.value = { kind: 'failed', reason: 'peer has no usable address' }
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
    // Pairing result comes back via listenPairingResult; on success we'll
    // open the data link from the handler below.
  } catch (e) {
    status.value = { kind: 'failed', reason: String(e) }
  }
  // Stash the data address so the result handler can use it.
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
      // Pair OK. If we're the joiner, open the data link to peer.
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
        // We were the responder — link is already on (we called startLink earlier).
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
  <section class="space-y-4">
    <!-- Identity card -->
    <div
      v-if="identity"
      class="rounded border border-zinc-800 bg-zinc-900/40 p-3 space-y-1.5"
    >
      <div class="flex items-center justify-between">
        <div class="text-[10px] uppercase tracking-widest text-zinc-500">
          This host
        </div>
        <div class="text-xs text-zinc-300">{{ identity.instance_name }}</div>
      </div>
      <div class="flex items-center justify-between gap-2">
        <div class="text-[10px] text-zinc-500">id</div>
        <div class="font-mono text-[11px] text-zinc-400 truncate flex-1 text-right">
          {{ identity.host_id_hex.slice(0, 24) }}…
        </div>
        <button
          class="text-[10px] px-1.5 py-0.5 rounded border border-zinc-700 text-zinc-400 hover:bg-zinc-800"
          @click="copy(identity.host_id_hex)"
        >
          copy
        </button>
      </div>
    </div>

    <!-- Active linked session -->
    <div v-if="linkActive && status.kind === 'linked'" class="space-y-3">
      <div class="flex items-center justify-between">
        <h2 class="text-sm uppercase tracking-widest text-emerald-400">
          {{
            link.role === 'sender' ? 'Joined session' : 'Hosting session'
          }}
        </h2>
        <button
          class="text-xs px-2 py-1 rounded border border-zinc-700 hover:bg-zinc-800"
          @click="stopSession"
        >
          {{ link.role === 'sender' ? 'Leave' : 'Stop' }}
        </button>
      </div>
      <div class="text-zinc-400 text-sm">
        <span class="text-zinc-500">peer:</span>
        <span class="text-zinc-200 ml-2">{{ link.peer || '—' }}</span>
      </div>
      <div class="grid grid-cols-2 gap-3">
        <div class="border border-zinc-800 rounded p-3">
          <div class="text-[10px] text-zinc-500 uppercase tracking-widest">
            latency p50
          </div>
          <div class="text-2xl tabular-nums">
            {{ p50ms }} <span class="text-sm text-zinc-500">ms</span>
          </div>
        </div>
        <div class="border border-zinc-800 rounded p-3">
          <div class="text-[10px] text-zinc-500 uppercase tracking-widest">
            latency p99
          </div>
          <div class="text-2xl tabular-nums">
            {{ p99ms }} <span class="text-sm text-zinc-500">ms</span>
          </div>
        </div>
        <div class="border border-zinc-800 rounded p-3">
          <div class="text-[10px] text-zinc-500 uppercase tracking-widest">
            events/sec
          </div>
          <div class="text-2xl tabular-nums">{{ link.eps }}</div>
        </div>
        <div class="border border-zinc-800 rounded p-3">
          <div class="text-[10px] text-zinc-500 uppercase tracking-widest">
            clock offset
          </div>
          <div class="text-2xl tabular-nums">
            {{ offsetMs }} <span class="text-sm text-zinc-500">ms</span>
          </div>
        </div>
      </div>
      <p class="text-xs text-zinc-600 leading-relaxed">
        Kill switch:
        <kbd class="px-1 bg-zinc-800 rounded text-zinc-300">Ctrl + ⌘ + ⇧ + Esc</kbd>
        (mac) /
        <kbd class="px-1 bg-zinc-800 rounded text-zinc-300">Ctrl + Win + ⇧ + Esc</kbd>
        (Win)
      </p>
    </div>

    <!-- Hosting (waiting for someone to join) -->
    <div v-else-if="status.kind === 'hosting'" class="space-y-3">
      <h2 class="text-sm uppercase tracking-widest text-zinc-400">
        Session live — share this code
      </h2>
      <div class="border border-zinc-800 rounded overflow-hidden">
        <div
          class="text-4xl tabular-nums tracking-[0.25em] text-center py-7 font-light"
        >
          {{ formattedCode(status.code) }}
        </div>
        <div v-if="codeRemaining !== null" class="h-1 bg-zinc-800">
          <div
            class="h-full bg-blue-500 transition-[width] duration-1000 ease-linear"
            :style="{ width: codeProgress * 100 + '%' }"
          />
        </div>
      </div>
      <p class="text-xs text-zinc-500 leading-relaxed">
        Open MouseFly on the other host and join this session.
        <span v-if="codeRemaining !== null">
          Code refreshes in
          <span class="text-zinc-300 font-mono">{{ codeRemainingLabel }}</span
          >.
        </span>
        <span v-else>Code never expires until you stop the session.</span>
      </p>
      <button
        class="text-xs px-2 py-1 rounded border border-zinc-700 hover:bg-zinc-800"
        @click="stopSession"
      >
        Stop session
      </button>
    </div>

    <!-- Joining: enter code -->
    <div v-else-if="status.kind === 'joining'" class="space-y-3">
      <h2 class="text-sm uppercase tracking-widest text-zinc-400">
        Join <span class="text-zinc-200 normal-case">{{ status.peerLabel }}</span>
      </h2>
      <input
        v-model="joinCode"
        class="w-full border border-zinc-800 rounded p-3 text-2xl tabular-nums tracking-widest text-center bg-zinc-900 focus:border-blue-700 outline-none"
        placeholder="123 456"
        autofocus
      />
      <div class="flex gap-2">
        <button
          class="flex-1 px-3 py-2 rounded bg-blue-700/40 border border-blue-700 hover:bg-blue-700/60 disabled:opacity-50 transition-colors"
          :disabled="joinCode.replace(/\s+/g, '').length < 6"
          @click="submitJoin"
        >
          Submit
        </button>
        <button
          class="px-3 py-2 rounded border border-zinc-700 hover:bg-zinc-800"
          @click="status = { kind: 'idle' }"
        >
          Cancel
        </button>
      </div>
    </div>

    <!-- Failed -->
    <div v-else-if="status.kind === 'failed'" class="space-y-3">
      <h2 class="text-sm uppercase tracking-widest text-red-400">Failed</h2>
      <div class="border border-red-800 bg-red-900/30 rounded p-3 text-sm">
        {{ status.reason }}
      </div>
      <button
        class="text-xs px-2 py-1 rounded border border-zinc-700 hover:bg-zinc-800"
        @click="status = { kind: 'idle' }"
      >
        Try again
      </button>
    </div>

    <!-- Idle: setup OR join entry -->
    <div v-else class="space-y-4">
      <!-- Setup form -->
      <div class="border border-zinc-800 rounded p-4 space-y-3">
        <h2 class="text-sm uppercase tracking-widest text-zinc-400">
          Set up a new session
        </h2>
        <p class="text-xs text-zinc-500 leading-relaxed">
          Hosts the link on this device. Share the code that appears with
          another host so it can join.
        </p>

        <label class="flex items-center gap-2 text-xs text-zinc-300">
          <input
            v-model="useCustomCode"
            type="checkbox"
            class="accent-blue-600"
          />
          Use a custom code
        </label>
        <div v-if="useCustomCode" class="space-y-1">
          <input
            v-model="customCode"
            class="w-full bg-zinc-900 border border-zinc-800 rounded px-2 py-1.5 text-xs font-mono"
            placeholder="≥6 letters/digits, e.g. summer42"
            @blur="validateCustomCode"
          />
          <div v-if="customCodeError" class="text-[11px] text-red-400">
            {{ customCodeError }}
          </div>
        </div>

        <div class="flex items-center gap-2 text-xs">
          <span class="text-zinc-500 uppercase tracking-widest text-[10px]"
            >refresh</span
          >
          <select
            v-model="ttlChoice"
            :disabled="useCustomCode"
            class="bg-zinc-900 border border-zinc-800 rounded px-2 py-1 text-xs disabled:opacity-50"
          >
            <option :value="5">every 5 min</option>
            <option :value="30">every 30 min</option>
            <option :value="60">every 1 h</option>
            <option value="never">never</option>
          </select>
          <span v-if="useCustomCode" class="text-[11px] text-zinc-600">
            (custom code, no refresh)
          </span>
        </div>

        <details
          class="border border-zinc-800 rounded text-xs"
          :open="advancedOpen"
          @toggle="advancedOpen = ($event.target as HTMLDetailsElement).open"
        >
          <summary class="px-3 py-2 cursor-pointer text-zinc-400 hover:text-zinc-200">
            Advanced
          </summary>
          <div class="p-3 pt-0 space-y-2">
            <label class="block text-[10px] uppercase tracking-widest text-zinc-500"
              >Listen address</label
            >
            <input
              v-model="listenAddr"
              class="w-full bg-zinc-900 border border-zinc-800 rounded px-2 py-1.5 text-xs font-mono"
              placeholder="0.0.0.0:7878"
            />
            <label class="flex items-center gap-2 text-xs text-zinc-400">
              <input v-model="enableInject" type="checkbox" class="accent-emerald-600" />
              Inject events into this OS
            </label>
          </div>
        </details>

        <button
          class="w-full px-3 py-2 rounded bg-emerald-700/40 border border-emerald-700 hover:bg-emerald-700/60 text-emerald-200 transition-colors"
          @click="setupSession"
        >
          Start session
        </button>
      </div>

      <!-- Discovered sessions -->
      <div class="border border-zinc-800 rounded p-4 space-y-3">
        <div class="flex items-center justify-between">
          <h2 class="text-sm uppercase tracking-widest text-zinc-400">
            Or join a session on the LAN
          </h2>
          <span
            v-if="visiblePeers.length"
            class="text-[10px] text-zinc-500"
            >{{ visiblePeers.length }} found</span
          >
        </div>
        <ul v-if="visiblePeers.length" class="space-y-2">
          <li
            v-for="peer in visiblePeers"
            :key="peer.instance_name"
            class="flex items-center justify-between border border-zinc-800 rounded p-3 hover:border-zinc-700 transition-colors"
          >
            <div class="min-w-0">
              <div class="text-sm truncate">{{ peer.instance_name }}</div>
              <div class="text-xs text-zinc-500 font-mono truncate">
                {{
                  formatPeerAddr(
                    pickPeerIp(peer.addrs) ?? peer.addrs[0] ?? '?',
                    peer.data_port || peer.port,
                  )
                }}
                · fp {{ peer.fingerprint_hex.slice(0, 12) }}…
              </div>
            </div>
            <button
              class="ml-3 text-xs px-2 py-1 rounded bg-blue-700/40 border border-blue-700 hover:bg-blue-700/60 transition-colors"
              @click="pickPeer(peer)"
            >
              Join
            </button>
          </li>
        </ul>
        <p v-else class="text-xs text-zinc-500">
          Scanning… both sides need to be on the same LAN.
        </p>
        <details class="border border-zinc-800 rounded text-xs">
          <summary class="px-3 py-2 cursor-pointer text-zinc-400 hover:text-zinc-200">
            Join by address instead
          </summary>
          <div class="p-3 pt-0 space-y-2">
            <input
              v-model="manualPeer"
              class="w-full bg-zinc-900 border border-zinc-800 rounded px-2 py-1.5 text-xs font-mono"
              placeholder="192.168.1.5:7879"
            />
            <button
              class="w-full text-xs px-2 py-1.5 rounded bg-blue-700/40 border border-blue-700 hover:bg-blue-700/60 disabled:opacity-50"
              :disabled="!manualPeer.trim()"
              @click="pickManual"
            >
              Continue
            </button>
          </div>
        </details>
      </div>

      <!-- Paired peers (memory of past joins) -->
      <div v-if="pairing.paired.length" class="space-y-2">
        <h2 class="text-sm uppercase tracking-widest text-zinc-400">
          Previously paired
        </h2>
        <ul class="space-y-1 text-xs font-mono text-zinc-500">
          <li
            v-for="p in pairing.paired"
            :key="p.host_id_hex"
            class="flex items-center justify-between gap-2"
          >
            <span class="truncate">{{ p.instance_name }}</span>
            <span class="text-zinc-600">{{ p.host_id_hex.slice(0, 10) }}…</span>
          </li>
        </ul>
      </div>
    </div>
  </section>
</template>
