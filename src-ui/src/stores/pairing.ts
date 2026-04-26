import { defineStore } from 'pinia'
import { ref } from 'vue'

export interface DiscoveredPeer {
  instance_name: string
  addrs: string[]
  port: number
  fingerprint_hex: string
  host_id_hex: string
  is_self: boolean
}

export interface PairedPeer {
  host_id_hex: string
  instance_name: string
  cert_fingerprint_hex: string
  paired_at_unix: number
}

export type PairingState =
  | { kind: 'idle' }
  | { kind: 'awaiting'; code: string }                     // we're showing a code
  | { kind: 'entering'; peer: DiscoveredPeer }              // we're entering a code
  | { kind: 'in-flight'; peer: string }                     // handshake running
  | { kind: 'success'; peer: PairedPeer }
  | { kind: 'failed'; reason: string }

export const usePairingStore = defineStore('pairing', () => {
  const discovered = ref<DiscoveredPeer[]>([])
  const paired = ref<PairedPeer[]>([])
  const state = ref<PairingState>({ kind: 'idle' })
  return { discovered, paired, state }
})
