import { listen, type UnlistenFn } from '@tauri-apps/api/event'

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
