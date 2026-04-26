import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { DiscoveredPeer, PairedPeer } from '../stores/pairing'

// Wire-format-matched payloads. Keep in sync with crates/mousefly-app/src/main.rs.

export type RoleEvent =
  | { kind: 'sender'; peer: string }
  | { kind: 'receiver'; listen: string; inject: boolean }

export interface LinkHealthEvent {
  role: 'sender' | 'receiver'
  p50_us: number
  p99_us: number
  events_per_sec: number
  clock_offset_ns: number
}

export type StatusSeverity = 'info' | 'warn' | 'error'

export interface LinkStatusEvent {
  severity: StatusSeverity
  text: string
}

export function listenRole(cb: (r: RoleEvent) => void): Promise<UnlistenFn> {
  return listen<RoleEvent>('role', (e) => cb(e.payload))
}

export function listenLinkHealth(cb: (h: LinkHealthEvent) => void): Promise<UnlistenFn> {
  return listen<LinkHealthEvent>('link-health', (e) => cb(e.payload))
}

export function listenLinkStatus(cb: (s: LinkStatusEvent) => void): Promise<UnlistenFn> {
  return listen<LinkStatusEvent>('link-status', (e) => cb(e.payload))
}

// --- Pairing IPC -----------------------------------------------------------

export interface PairingCodeEvent {
  code: string
  expires_unix: number
}

export interface PairingResultEvent {
  ok: boolean
  peer?: PairedPeer
  reason?: string
  verification_sas?: string
}

export interface PairingLockedEvent {
  reason: string
}

export interface LocalIdentity {
  host_id_hex: string
  instance_name: string
  cert_fingerprint_hex: string
}

export function listenDiscoveredPeers(
  cb: (peers: DiscoveredPeer[]) => void,
): Promise<UnlistenFn> {
  return listen<DiscoveredPeer[]>('discovered-peers', (e) => cb(e.payload))
}

export function listenPairingCode(
  cb: (e: PairingCodeEvent) => void,
): Promise<UnlistenFn> {
  return listen<PairingCodeEvent>('pairing-code', (e) => cb(e.payload))
}

export function listenPairingResult(
  cb: (e: PairingResultEvent) => void,
): Promise<UnlistenFn> {
  return listen<PairingResultEvent>('pairing-result', (e) => cb(e.payload))
}

export async function listPairedPeers(): Promise<PairedPeer[]> {
  return await invoke<PairedPeer[]>('list_paired_peers')
}

export async function getLocalIdentity(): Promise<LocalIdentity> {
  return await invoke<LocalIdentity>('get_local_identity')
}

export function listenPairingLocked(
  cb: (e: PairingLockedEvent) => void,
): Promise<UnlistenFn> {
  return listen<PairingLockedEvent>('pairing-locked', (e) => cb(e.payload))
}

export async function startPairResponder(): Promise<string> {
  return await invoke<string>('start_pair_responder')
}

export async function startPairInitiator(
  addr: string,
  code: string,
): Promise<void> {
  await invoke('start_pair_initiator', { addr, code })
}

export async function cancelPairing(): Promise<void> {
  await invoke('cancel_pairing')
}

// --- Layout IPC ------------------------------------------------------------

export interface WireMonitorId {
  0: number
}

export interface WireMonitor {
  id: WireMonitorId
  name: string
  logical_size_px: [number, number]
  scale_factor: number
  physical_size_mm: [number, number] | null
  position_in_local_vd: [number, number]
  primary: boolean
}

export interface LayoutEvent {
  side: 'local' | 'remote'
  monitors: WireMonitor[]
}

export function listenLayout(cb: (e: LayoutEvent) => void): Promise<UnlistenFn> {
  return listen<LayoutEvent>('layout', (e) => cb(e.payload))
}
