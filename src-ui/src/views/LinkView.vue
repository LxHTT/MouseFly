<script setup lang="ts">
import { computed, ref } from 'vue'
import { useLinkStore } from '../stores/link'
import { usePairingStore } from '../stores/pairing'
import { startLink, stopLink } from '../ipc'

const link = useLinkStore()
const pairing = usePairingStore()

const listenAddr = ref('0.0.0.0:7878')
const peerAddr = ref('')
const enableInject = ref(false)
const busy = ref(false)
const errMsg = ref('')

const p50ms = computed(() => (link.p50us / 1000).toFixed(1))
const p99ms = computed(() => (link.p99us / 1000).toFixed(1))
const offsetMs = computed(() => (link.offsetNs / 1e6).toFixed(2))
const isIdle = computed(() => link.role === 'idle' || link.role === 'connecting')

const roleClass = computed(() => {
  if (link.role === 'sender') return 'bg-blue-700/40 text-blue-200 border-blue-700'
  if (link.role === 'receiver') return 'bg-emerald-700/40 text-emerald-200 border-emerald-700'
  return 'bg-zinc-700/40 text-zinc-300 border-zinc-700'
})
const statusClass = computed(() => {
  if (link.statusSeverity === 'error') return 'bg-red-900/40 border-red-800 text-red-200'
  if (link.statusSeverity === 'warn') return 'bg-amber-900/40 border-amber-800 text-amber-200'
  return 'bg-zinc-900/60 border-zinc-800 text-zinc-300'
})

async function startReceiver() {
  errMsg.value = ''
  busy.value = true
  try {
    await startLink({ kind: 'receiver', listen: listenAddr.value, inject: enableInject.value })
  } catch (e) {
    errMsg.value = String(e)
  } finally {
    busy.value = false
  }
}

async function startSender(addr?: string) {
  const target = addr ?? peerAddr.value.trim()
  if (!target) return
  errMsg.value = ''
  busy.value = true
  try {
    await startLink({ kind: 'sender', peer: target })
  } catch (e) {
    errMsg.value = String(e)
  } finally {
    busy.value = false
  }
}

async function disconnect() {
  busy.value = true
  try {
    await stopLink()
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <section class="space-y-4">
    <header class="flex items-center justify-between">
      <h2 class="text-sm uppercase tracking-widest text-zinc-400">Link</h2>
      <span
        :class="['px-2 py-1 rounded text-[10px] uppercase tracking-widest border', roleClass]"
      >
        {{ link.role }}
      </span>
    </header>

    <!-- Idle: pick a role -->
    <div v-if="isIdle" class="space-y-4">
      <div class="border border-zinc-800 rounded p-4 space-y-3">
        <div class="text-xs uppercase tracking-widest text-zinc-500">Wait for incoming</div>
        <div class="space-y-2">
          <input
            v-model="listenAddr"
            class="w-full bg-zinc-900 border border-zinc-800 rounded px-2 py-1.5 text-xs font-mono focus:border-emerald-700 outline-none"
            placeholder="0.0.0.0:7878"
          />
          <label class="flex items-center gap-2 text-xs text-zinc-400">
            <input v-model="enableInject" type="checkbox" class="accent-emerald-600" />
            Inject events into this OS (turn on only on a real second machine)
          </label>
          <button
            class="w-full px-3 py-2 rounded bg-emerald-700/40 border border-emerald-700 hover:bg-emerald-700/60 disabled:opacity-50 transition-colors"
            :disabled="busy"
            @click="startReceiver"
          >
            Start receiver
          </button>
        </div>
      </div>

      <div class="border border-zinc-800 rounded p-4 space-y-3">
        <div class="text-xs uppercase tracking-widest text-zinc-500">Connect to a peer</div>
        <ul v-if="pairing.paired.length" class="space-y-1.5">
          <li
            v-for="p in pairing.paired"
            :key="p.host_id_hex"
            class="flex items-center justify-between gap-2 text-xs"
          >
            <span class="font-mono text-zinc-400 truncate flex-1">{{ p.instance_name }}</span>
            <button
              class="px-2 py-1 rounded bg-blue-700/40 border border-blue-700 hover:bg-blue-700/60 disabled:opacity-50 transition-colors"
              :disabled="busy"
              @click="startSender(`${p.instance_name}:7878`)"
              :title="'No address — click to enter manually'"
            >
              Connect
            </button>
          </li>
        </ul>
        <p v-else class="text-xs text-zinc-500">
          No paired peers yet. Pair via the
          <span class="text-zinc-300">Pair</span> tab first.
        </p>
        <div class="space-y-2 pt-1 border-t border-zinc-800">
          <div class="text-[10px] uppercase tracking-widest text-zinc-600">or by address</div>
          <input
            v-model="peerAddr"
            class="w-full bg-zinc-900 border border-zinc-800 rounded px-2 py-1.5 text-xs font-mono focus:border-blue-700 outline-none"
            placeholder="192.168.1.5:7878"
          />
          <button
            class="w-full px-3 py-2 rounded bg-blue-700/40 border border-blue-700 hover:bg-blue-700/60 disabled:opacity-50 transition-colors"
            :disabled="busy || !peerAddr.trim()"
            @click="startSender()"
          >
            Connect to address
          </button>
        </div>
      </div>

      <div v-if="errMsg" :class="['rounded border px-3 py-2 text-xs', statusClass]">
        {{ errMsg }}
      </div>
      <div v-else :class="['rounded border px-3 py-2 text-xs leading-relaxed', statusClass]">
        {{ link.statusText }}
      </div>
    </div>

    <!-- Active: show health + disconnect -->
    <div v-else class="space-y-4">
      <div class="text-zinc-400 text-sm space-y-1">
        <div>
          <span class="text-zinc-500">peer:</span>
          <span class="text-zinc-200 ml-2">{{ link.peer || '—' }}</span>
        </div>
        <div v-if="link.role === 'receiver'">
          <span class="text-zinc-500">inject:</span>
          <span :class="['ml-2', link.inject ? 'text-emerald-400' : 'text-zinc-400']">
            {{ link.inject ? 'on' : 'off (loopback safe)' }}
          </span>
        </div>
      </div>

      <div class="grid grid-cols-2 gap-3">
        <div class="border border-zinc-800 rounded p-3">
          <div class="text-[10px] text-zinc-500 uppercase tracking-widest">latency p50</div>
          <div class="text-2xl tabular-nums">
            {{ p50ms }} <span class="text-sm text-zinc-500">ms</span>
          </div>
        </div>
        <div class="border border-zinc-800 rounded p-3">
          <div class="text-[10px] text-zinc-500 uppercase tracking-widest">latency p99</div>
          <div class="text-2xl tabular-nums">
            {{ p99ms }} <span class="text-sm text-zinc-500">ms</span>
          </div>
        </div>
        <div class="border border-zinc-800 rounded p-3">
          <div class="text-[10px] text-zinc-500 uppercase tracking-widest">events/sec</div>
          <div class="text-2xl tabular-nums">{{ link.eps }}</div>
        </div>
        <div class="border border-zinc-800 rounded p-3">
          <div class="text-[10px] text-zinc-500 uppercase tracking-widest">clock offset</div>
          <div class="text-2xl tabular-nums">
            {{ offsetMs }} <span class="text-sm text-zinc-500">ms</span>
          </div>
        </div>
      </div>

      <div :class="['rounded border px-3 py-2 text-xs leading-relaxed', statusClass]">
        {{ link.statusText }}
      </div>

      <button
        class="text-xs px-2 py-1 rounded border border-zinc-700 hover:bg-zinc-800"
        :disabled="busy"
        @click="disconnect"
      >
        Disconnect
      </button>

      <p class="text-xs text-zinc-600 leading-relaxed">
        Kill switch:
        <kbd class="px-1 bg-zinc-800 rounded text-zinc-300">Ctrl + ⌘ + ⇧ + Esc</kbd>
        (mac) /
        <kbd class="px-1 bg-zinc-800 rounded text-zinc-300">Ctrl + Win + ⇧ + Esc</kbd>
        (Windows)
      </p>
    </div>
  </section>
</template>
