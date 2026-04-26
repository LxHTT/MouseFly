<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import type { UnlistenFn } from '@tauri-apps/api/event'
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
  startPairInitiator,
  startPairResponder,
  type LocalIdentity,
} from '../ipc'

const pairing = usePairingStore()
const codeInput = ref('')
const manualAddr = ref('')
const showManual = ref(false)
const identity = ref<LocalIdentity | null>(null)
const now = ref(Math.floor(Date.now() / 1000))
let tickHandle: ReturnType<typeof setInterval> | null = null
let unlistenDiscovered: UnlistenFn | null = null
let unlistenCode: UnlistenFn | null = null
let unlistenResult: UnlistenFn | null = null
let unlistenLocked: UnlistenFn | null = null

onMounted(async () => {
  identity.value = await getLocalIdentity().catch(() => null)
  unlistenDiscovered = await listenDiscoveredPeers((peers) => {
    pairing.discovered = peers
  })
  unlistenCode = await listenPairingCode((e) => {
    pairing.state = { kind: 'awaiting', code: e.code, expiresUnix: e.expires_unix }
  })
  unlistenResult = await listenPairingResult((e) => {
    if (e.ok && e.peer) {
      pairing.state = {
        kind: 'success',
        peer: e.peer,
        verificationSas: e.verification_sas ?? '',
      }
      listPairedPeers().then((p) => (pairing.paired = p))
    } else {
      pairing.state = { kind: 'failed', reason: e.reason ?? 'pairing failed' }
    }
  })
  unlistenLocked = await listenPairingLocked((e) => {
    pairing.state = { kind: 'failed', reason: e.reason }
  })
  pairing.paired = await listPairedPeers().catch(() => [])
  tickHandle = setInterval(() => {
    now.value = Math.floor(Date.now() / 1000)
  }, 1000)
})

onBeforeUnmount(() => {
  unlistenDiscovered?.()
  unlistenCode?.()
  unlistenResult?.()
  unlistenLocked?.()
  if (tickHandle !== null) clearInterval(tickHandle)
})

async function startResponder() {
  pairing.state = { kind: 'in-flight', peer: 'awaiting peer…' }
  try {
    await startPairResponder()
  } catch (e) {
    pairing.state = { kind: 'failed', reason: String(e) }
  }
}

function pickPeer(peer: DiscoveredPeer) {
  const ip = pickPeerIp(peer.addrs)
  if (!ip) {
    pairing.state = { kind: 'failed', reason: 'peer has no usable address' }
    return
  }
  const addr = formatPeerAddr(ip, peer.port)
  pairing.state = { kind: 'entering', peerLabel: peer.instance_name, peerAddr: addr }
  codeInput.value = ''
}

function pickManual() {
  if (!manualAddr.value.trim()) return
  pairing.state = {
    kind: 'entering',
    peerLabel: manualAddr.value.trim(),
    peerAddr: manualAddr.value.trim(),
  }
  codeInput.value = ''
}

async function submitCode() {
  if (pairing.state.kind !== 'entering') return
  const peer = pairing.state.peerAddr
  pairing.state = { kind: 'in-flight', peer }
  try {
    await startPairInitiator(peer, codeInput.value.replace(/\s+/g, ''))
  } catch (e) {
    pairing.state = { kind: 'failed', reason: String(e) }
  }
}

async function cancel() {
  await cancelPairing().catch(() => undefined)
  pairing.state = { kind: 'idle' }
  codeInput.value = ''
  showManual.value = false
}

const visible = computed(() => pairing.discovered.filter((p) => !p.is_self))

const formattedCode = (code: string) =>
  code.length === 6 ? `${code.slice(0, 3)} ${code.slice(3)}` : code

const codeRemaining = computed(() => {
  if (pairing.state.kind !== 'awaiting') return 0
  return Math.max(0, pairing.state.expiresUnix - now.value)
})

const codeRemainingLabel = computed(() => {
  const s = codeRemaining.value
  const m = Math.floor(s / 60)
  const r = s % 60
  return `${m}:${String(r).padStart(2, '0')}`
})

const codeProgress = computed(() => {
  if (pairing.state.kind !== 'awaiting') return 0
  const total = 5 * 60
  return Math.min(1, codeRemaining.value / total)
})

async function copy(text: string) {
  try {
    await navigator.clipboard.writeText(text)
  } catch (_e) {
    /* ignore */
  }
}
</script>

<template>
  <section class="space-y-4">
    <!-- Identity card -->
    <div
      v-if="identity"
      class="rounded border border-zinc-800 bg-zinc-900/40 p-3 space-y-1.5"
    >
      <div class="flex items-center justify-between">
        <div class="text-[10px] uppercase tracking-widest text-zinc-500">This host</div>
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
      <div class="flex items-center justify-between gap-2">
        <div class="text-[10px] text-zinc-500">fp</div>
        <div class="font-mono text-[11px] text-zinc-400 truncate flex-1 text-right">
          {{ identity.cert_fingerprint_hex.slice(0, 24) }}…
        </div>
        <button
          class="text-[10px] px-1.5 py-0.5 rounded border border-zinc-700 text-zinc-400 hover:bg-zinc-800"
          @click="copy(identity.cert_fingerprint_hex)"
        >
          copy
        </button>
      </div>
    </div>

    <!-- Idle: discovered peers + manual entry -->
    <div v-if="pairing.state.kind === 'idle'" class="space-y-3">
      <div class="flex items-center justify-between">
        <h2 class="text-sm uppercase tracking-widest text-zinc-400">Discovered on LAN</h2>
        <button
          class="text-xs px-2 py-1 rounded border border-zinc-700 hover:bg-zinc-800"
          @click="startResponder"
        >
          Show pairing code
        </button>
      </div>
      <ul v-if="visible.length" class="space-y-2">
        <li
          v-for="peer in visible"
          :key="peer.instance_name"
          class="flex items-center justify-between border border-zinc-800 rounded p-3 hover:border-zinc-700 transition-colors"
        >
          <div class="min-w-0">
            <div class="text-sm truncate">{{ peer.instance_name }}</div>
            <div class="text-xs text-zinc-500 font-mono truncate">
              {{ formatPeerAddr(pickPeerIp(peer.addrs) ?? peer.addrs[0] ?? '?', peer.port) }} ·
              fp {{ peer.fingerprint_hex.slice(0, 12) }}…
            </div>
          </div>
          <button
            class="ml-3 text-xs px-2 py-1 rounded bg-blue-700/40 border border-blue-700 hover:bg-blue-700/60 transition-colors"
            @click="pickPeer(peer)"
          >
            Pair
          </button>
        </li>
      </ul>
      <p v-else class="text-xs text-zinc-500">
        No peers seen yet. Both sides need MouseFly running on the same LAN.
      </p>

      <details
        class="border border-zinc-800 rounded text-xs"
        :open="showManual"
        @toggle="showManual = ($event.target as HTMLDetailsElement).open"
      >
        <summary class="px-3 py-2 cursor-pointer text-zinc-400 hover:text-zinc-200">
          Pair manually by IP
        </summary>
        <div class="p-3 pt-0 space-y-2">
          <input
            v-model="manualAddr"
            class="w-full bg-zinc-900 border border-zinc-800 rounded px-2 py-1.5 text-xs font-mono"
            placeholder="192.168.1.5:7879"
          />
          <button
            class="w-full text-xs px-2 py-1.5 rounded bg-blue-700/40 border border-blue-700 hover:bg-blue-700/60 disabled:opacity-50"
            :disabled="!manualAddr.trim()"
            @click="pickManual"
          >
            Continue
          </button>
        </div>
      </details>
    </div>

    <!-- Awaiting: showing code with countdown -->
    <div v-else-if="pairing.state.kind === 'awaiting'" class="space-y-3">
      <h2 class="text-sm uppercase tracking-widest text-zinc-400">Pairing code</h2>
      <div class="border border-zinc-800 rounded overflow-hidden">
        <div class="text-5xl tabular-nums tracking-[0.3em] text-center py-7 font-light">
          {{ formattedCode(pairing.state.code) }}
        </div>
        <div class="h-1 bg-zinc-800">
          <div
            class="h-full bg-blue-500 transition-[width] duration-1000 ease-linear"
            :style="{ width: codeProgress * 100 + '%' }"
          />
        </div>
      </div>
      <p class="text-xs text-zinc-500 leading-relaxed">
        Enter this code on the other host. One-time use, expires in
        <span class="text-zinc-300 font-mono">{{ codeRemainingLabel }}</span>.
      </p>
      <button
        class="text-xs px-2 py-1 rounded border border-zinc-700 hover:bg-zinc-800"
        @click="cancel"
      >
        Cancel
      </button>
    </div>

    <!-- Entering: type code for chosen peer -->
    <div v-else-if="pairing.state.kind === 'entering'" class="space-y-3">
      <h2 class="text-sm uppercase tracking-widest text-zinc-400">
        Pair with <span class="text-zinc-200 normal-case">{{ pairing.state.peerLabel }}</span>
      </h2>
      <input
        v-model="codeInput"
        class="w-full border border-zinc-800 rounded p-3 text-2xl tabular-nums tracking-widest text-center bg-zinc-900 focus:border-blue-700 outline-none"
        inputmode="numeric"
        maxlength="7"
        placeholder="123 456"
        autofocus
      />
      <div class="flex gap-2">
        <button
          class="flex-1 px-3 py-2 rounded bg-blue-700/40 border border-blue-700 hover:bg-blue-700/60 disabled:opacity-50 transition-colors"
          :disabled="codeInput.replace(/\s+/g, '').length !== 6"
          @click="submitCode"
        >
          Submit
        </button>
        <button
          class="px-3 py-2 rounded border border-zinc-700 hover:bg-zinc-800"
          @click="cancel"
        >
          Cancel
        </button>
      </div>
    </div>

    <!-- In-flight -->
    <div v-else-if="pairing.state.kind === 'in-flight'" class="space-y-2">
      <h2 class="text-sm uppercase tracking-widest text-zinc-400">Pairing…</h2>
      <div class="flex items-center gap-2 text-zinc-300">
        <span class="inline-block w-2 h-2 rounded-full bg-blue-400 animate-pulse" />
        <span class="text-xs">{{ pairing.state.peer }}</span>
      </div>
    </div>

    <!-- Success -->
    <div v-else-if="pairing.state.kind === 'success'" class="space-y-3">
      <h2 class="text-sm uppercase tracking-widest text-emerald-400">Paired</h2>
      <div class="border border-emerald-800 bg-emerald-900/30 rounded p-3 text-sm space-y-2">
        <div class="font-medium">{{ pairing.state.peer.instance_name }}</div>
        <div class="text-xs text-zinc-500 font-mono">
          host id {{ pairing.state.peer.host_id_hex.slice(0, 16) }}…
        </div>
      </div>
      <div
        v-if="pairing.state.verificationSas"
        class="border border-zinc-800 rounded p-3 space-y-1.5"
      >
        <div class="text-[10px] uppercase tracking-widest text-zinc-500">
          Verification code
        </div>
        <div class="font-mono text-2xl tabular-nums tracking-widest text-emerald-300">
          {{ pairing.state.verificationSas }}
        </div>
        <p class="text-[11px] text-zinc-500 leading-relaxed">
          Both hosts should show the same string. If they don't,
          unpair immediately — someone may be on the wire.
        </p>
      </div>
      <button
        class="text-xs px-2 py-1 rounded border border-zinc-700 hover:bg-zinc-800"
        @click="cancel"
      >
        Done
      </button>
    </div>

    <!-- Failed -->
    <div v-else-if="pairing.state.kind === 'failed'" class="space-y-3">
      <h2 class="text-sm uppercase tracking-widest text-red-400">Pairing failed</h2>
      <div class="border border-red-800 bg-red-900/30 rounded p-3 text-sm">
        {{ pairing.state.reason }}
      </div>
      <button
        class="text-xs px-2 py-1 rounded border border-zinc-700 hover:bg-zinc-800"
        @click="cancel"
      >
        Try again
      </button>
    </div>

    <hr v-if="pairing.paired.length" class="border-zinc-800" />

    <!-- Paired peers list -->
    <div v-if="pairing.paired.length" class="space-y-2">
      <h2 class="text-sm uppercase tracking-widest text-zinc-400">Paired peers</h2>
      <ul class="space-y-1">
        <li
          v-for="p in pairing.paired"
          :key="p.host_id_hex"
          class="text-xs font-mono text-zinc-500 flex items-center justify-between gap-2"
        >
          <span class="truncate">{{ p.instance_name }}</span>
          <span class="text-zinc-600">{{ p.host_id_hex.slice(0, 10) }}…</span>
        </li>
      </ul>
    </div>
  </section>
</template>
