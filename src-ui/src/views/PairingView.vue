<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { usePairingStore, type DiscoveredPeer } from '../stores/pairing'
import {
  cancelPairing,
  listenDiscoveredPeers,
  listenPairingCode,
  listenPairingResult,
  listPairedPeers,
  startPairInitiator,
  startPairResponder,
} from '../ipc'

const pairing = usePairingStore()
const codeInput = ref('')
const selectedPeer = ref<DiscoveredPeer | null>(null)
let unlistenDiscovered: UnlistenFn | null = null
let unlistenCode: UnlistenFn | null = null
let unlistenResult: UnlistenFn | null = null

onMounted(async () => {
  unlistenDiscovered = await listenDiscoveredPeers((peers) => {
    pairing.discovered = peers
  })
  unlistenCode = await listenPairingCode((e) => {
    pairing.state = { kind: 'awaiting', code: e.code }
  })
  unlistenResult = await listenPairingResult((e) => {
    if (e.ok && e.peer) {
      pairing.state = { kind: 'success', peer: e.peer }
      // Refresh paired list.
      listPairedPeers().then((p) => (pairing.paired = p))
    } else {
      pairing.state = { kind: 'failed', reason: e.reason ?? 'pairing failed' }
    }
  })
  pairing.paired = await listPairedPeers().catch(() => [])
})

onBeforeUnmount(() => {
  unlistenDiscovered?.()
  unlistenCode?.()
  unlistenResult?.()
})

async function startResponder() {
  pairing.state = { kind: 'in-flight', peer: 'awaiting peer…' }
  try {
    const code = await startPairResponder()
    pairing.state = { kind: 'awaiting', code }
  } catch (e) {
    pairing.state = { kind: 'failed', reason: String(e) }
  }
}

function pickPeer(peer: DiscoveredPeer) {
  selectedPeer.value = peer
  pairing.state = { kind: 'entering', peer }
  codeInput.value = ''
}

async function submitCode() {
  if (!selectedPeer.value) return
  const peer = selectedPeer.value
  const addr = `${peer.addrs[0]}:${peer.port}`
  pairing.state = { kind: 'in-flight', peer: addr }
  try {
    await startPairInitiator(addr, codeInput.value.replace(/\s+/g, ''))
  } catch (e) {
    pairing.state = { kind: 'failed', reason: String(e) }
  }
}

async function cancel() {
  await cancelPairing().catch(() => undefined)
  pairing.state = { kind: 'idle' }
  selectedPeer.value = null
  codeInput.value = ''
}

const visible = computed(() => pairing.discovered.filter((p) => !p.is_self))
const formattedCode = (code: string) =>
  code.length === 6 ? `${code.slice(0, 3)} ${code.slice(3)}` : code
</script>

<template>
  <section class="space-y-4">
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
          class="flex items-center justify-between border border-zinc-800 rounded p-3"
        >
          <div>
            <div class="text-sm">{{ peer.instance_name }}</div>
            <div class="text-xs text-zinc-500 font-mono">
              {{ peer.addrs[0] }}:{{ peer.port }} · fp {{ peer.fingerprint_hex.slice(0, 12) }}…
            </div>
          </div>
          <button
            class="text-xs px-2 py-1 rounded bg-blue-700/40 border border-blue-700 hover:bg-blue-700/60"
            @click="pickPeer(peer)"
          >
            Pair
          </button>
        </li>
      </ul>
      <p v-else class="text-xs text-zinc-500">No peers seen yet. Make sure both sides are on the same LAN.</p>
    </div>

    <div v-else-if="pairing.state.kind === 'awaiting'" class="space-y-3">
      <h2 class="text-sm uppercase tracking-widest text-zinc-400">Pairing code</h2>
      <div class="text-4xl tabular-nums tracking-widest text-center py-6 border border-zinc-800 rounded">
        {{ formattedCode(pairing.state.code) }}
      </div>
      <p class="text-xs text-zinc-500">Type this on the other host. Cancels when paired or after 5 minutes.</p>
      <button
        class="text-xs px-2 py-1 rounded border border-zinc-700 hover:bg-zinc-800"
        @click="cancel"
      >
        Cancel
      </button>
    </div>

    <div v-else-if="pairing.state.kind === 'entering'" class="space-y-3">
      <h2 class="text-sm uppercase tracking-widest text-zinc-400">
        Pair with {{ pairing.state.peer.instance_name }}
      </h2>
      <input
        v-model="codeInput"
        class="w-full border border-zinc-800 rounded p-3 text-2xl tabular-nums tracking-widest text-center bg-zinc-900"
        inputmode="numeric"
        maxlength="7"
        placeholder="123 456"
      />
      <div class="flex gap-2">
        <button
          class="flex-1 px-3 py-2 rounded bg-blue-700/40 border border-blue-700 hover:bg-blue-700/60 disabled:opacity-50"
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

    <div v-else-if="pairing.state.kind === 'in-flight'" class="space-y-2">
      <h2 class="text-sm uppercase tracking-widest text-zinc-400">Pairing…</h2>
      <p class="text-xs text-zinc-500">Talking to {{ pairing.state.peer }}.</p>
    </div>

    <div v-else-if="pairing.state.kind === 'success'" class="space-y-3">
      <h2 class="text-sm uppercase tracking-widest text-emerald-400">Paired</h2>
      <div class="border border-emerald-800 bg-emerald-900/30 rounded p-3 text-sm">
        <div>{{ pairing.state.peer.instance_name }}</div>
        <div class="text-xs text-zinc-500 font-mono mt-1">
          host id {{ pairing.state.peer.host_id_hex.slice(0, 12) }}…
        </div>
      </div>
      <button
        class="text-xs px-2 py-1 rounded border border-zinc-700 hover:bg-zinc-800"
        @click="cancel"
      >
        Done
      </button>
    </div>

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

    <hr class="border-zinc-800" />

    <div class="space-y-2">
      <h2 class="text-sm uppercase tracking-widest text-zinc-400">Paired peers</h2>
      <ul v-if="pairing.paired.length" class="space-y-1">
        <li
          v-for="p in pairing.paired"
          :key="p.host_id_hex"
          class="text-xs font-mono text-zinc-500"
        >
          {{ p.instance_name }} · {{ p.host_id_hex.slice(0, 12) }}…
        </li>
      </ul>
      <p v-else class="text-xs text-zinc-600">None yet.</p>
    </div>
  </section>
</template>
