import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { DiscoveredPeer, PairedPeer } from '../stores/pairing'

// Wire-format-matched payloads. Keep in sync with crates/mousefly-app/src/main.rs.

export type RoleEvent =
  | { kind: 'idle' }
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

export function listenLinkDropped(cb: () => void): Promise<UnlistenFn> {
  return listen('link-dropped', () => cb())
}

export function listenPeerAddr(cb: (addr: string) => void): Promise<UnlistenFn> {
  return listen<string>('peer-addr', (e) => cb(e.payload))
}

export interface LogEntryEvent {
  level: string
  message: string
}

export function listenLogEntry(cb: (e: LogEntryEvent) => void): Promise<UnlistenFn> {
  return listen<LogEntryEvent>('log-entry', (e) => cb(e.payload))
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

/// Pick the most-likely-routable address from an mDNS peer's address list.
/// Prefers IPv4, then routable IPv6 (excludes link-local fe80::/10 — that
/// needs a zone identifier we don't carry across the wire).
export function pickPeerIp(addrs: string[]): string | null {
  if (addrs.length === 0) return null
  const v4 = addrs.find((a) => /^\d+\.\d+\.\d+\.\d+$/.test(a))
  if (v4) return v4
  const routable = addrs.find((a) => a.includes(':') && !a.toLowerCase().startsWith('fe80'))
  if (routable) return routable
  return addrs[0]
}

/// Combine an IP and port into a parseable host:port string. IPv6 needs
/// square-bracket notation (`[fe80::1]:7878`), IPv4 doesn't.
export function formatPeerAddr(ip: string, port: number): string {
  return ip.includes(':') ? `[${ip}]:${port}` : `${ip}:${port}`
}

export async function getLocalIdentity(): Promise<LocalIdentity> {
  return await invoke<LocalIdentity>('get_local_identity')
}

export async function getPairingState(): Promise<PairingCodeEvent | null> {
  return await invoke<PairingCodeEvent | null>('get_pairing_state')
}

export function listenPairingLocked(
  cb: (e: PairingLockedEvent) => void,
): Promise<UnlistenFn> {
  return listen<PairingLockedEvent>('pairing-locked', (e) => cb(e.payload))
}

// --- Link role control -----------------------------------------------------

export type StartLinkRole =
  | { kind: 'idle' }
  | { kind: 'sender'; peer: string }
  | { kind: 'receiver'; listen: string; inject: boolean }

export async function startLink(role: StartLinkRole): Promise<void> {
  await invoke('start_link', { role })
}

export async function stopLink(): Promise<void> {
  await invoke('stop_link')
}

export async function currentRole(): Promise<RoleEvent> {
  return await invoke<RoleEvent>('current_role')
}

export async function checkPermissions(): Promise<boolean> {
  return await invoke<boolean>('check_permissions')
}

export async function requestPermissions(): Promise<boolean> {
  return await invoke<boolean>('request_permissions')
}

export async function startAdvertising(): Promise<void> {
  await invoke('start_advertising')
}

export async function stopAdvertising(): Promise<void> {
  await invoke('stop_advertising')
}

export interface StartResponderArgs {
  code?: string | null
  ttl_seconds?: number | null
}

export async function startPairResponder(
  args: StartResponderArgs = {},
): Promise<string> {
  return await invoke<string>('start_pair_responder', { args })
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

// Rust's `MonitorId(pub u64)` is a transparent newtype on the wire — serde
// emits the bare integer, not `{ "0": n }`. Older builds wrapped it; accept
// both shapes so a stale peer still maps cleanly.
export type WireMonitorId = number | { 0: number }

export interface WireMonitor {
  id: WireMonitorId
  name: string
  logical_size_px: [number, number]
  scale_factor: number
  physical_size_mm: [number, number] | null
  position_in_local_vd: [number, number]
  primary: boolean
}

export function monitorIdToString(id: WireMonitorId): string {
  const n = typeof id === 'number' ? id : (id?.[0] ?? 0)
  // u64 can exceed JS Number.MAX_SAFE_INTEGER but the Rust hash output is
  // serde-i64-clamped in practice; if it ever overflows we'd see precision
  // loss, not a crash.
  return n.toString(16)
}

export interface LayoutEvent {
  side: 'local' | 'remote'
  monitors: WireMonitor[]
}

export function listenLayout(cb: (e: LayoutEvent) => void): Promise<UnlistenFn> {
  return listen<LayoutEvent>('layout', (e) => cb(e.payload))
}

export function listenLayoutEditLock(cb: (editing: boolean) => void): Promise<UnlistenFn> {
  return listen<boolean>('layout-edit-lock', (e) => cb(e.payload))
}

export async function notifyLayoutEditing(editing: boolean): Promise<void> {
  await invoke('notify_layout_editing', { editing })
}
