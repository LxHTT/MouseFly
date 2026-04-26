import { defineStore } from 'pinia'
import { ref } from 'vue'

export type HostSide = 'local' | 'remote'

export interface CanvasMonitor {
  id: string
  name: string
  widthPx: number
  heightPx: number
  scale: number
  mmW: number | null
  mmH: number | null
  posX: number
  posY: number
  primary: boolean
}

export interface HostLayout {
  side: HostSide
  instanceName: string
  monitors: CanvasMonitor[]
  offsetX: number
  offsetY: number
}

export const useLayoutStore = defineStore('layout', () => {
  const local = ref<HostLayout | null>(null)
  const remote = ref<HostLayout | null>(null)

  function setHost(host: HostLayout) {
    const target = host.side === 'local' ? local : remote
    if (target.value) {
      host.offsetX = target.value.offsetX
      host.offsetY = target.value.offsetY
    }
    target.value = host
  }

  function moveHost(side: HostSide, dx: number, dy: number) {
    const target = side === 'local' ? local.value : remote.value
    if (!target) return
    target.offsetX += dx
    target.offsetY += dy
  }

  function setHostOffset(side: HostSide, x: number, y: number) {
    const target = side === 'local' ? local.value : remote.value
    if (!target) return
    target.offsetX = x
    target.offsetY = y
  }

  function resetOffsets() {
    if (local.value) {
      local.value.offsetX = 0
      local.value.offsetY = 0
    }
    if (remote.value) {
      const localBounds = local.value ? hostBounds(local.value) : null
      const gap = 100
      const startX = localBounds ? localBounds.maxX + gap : 0
      remote.value.offsetX = startX
      remote.value.offsetY = 0
    }
  }

  function __dev_mock__() {
    setHost({
      side: 'local',
      instanceName: 'Studio (Mac)',
      offsetX: 0,
      offsetY: 0,
      monitors: [
        {
          id: 'l0',
          name: 'Mon0',
          widthPx: 3840,
          heightPx: 2160,
          scale: 2,
          mmW: 600,
          mmH: 340,
          posX: 0,
          posY: 0,
          primary: true,
        },
        {
          id: 'l1',
          name: 'Mon1',
          widthPx: 2560,
          heightPx: 1440,
          scale: 1,
          mmW: 600,
          mmH: 340,
          posX: 3840,
          posY: 200,
          primary: false,
        },
      ],
    })
    setHost({
      side: 'remote',
      instanceName: 'Lap (Win)',
      offsetX: 0,
      offsetY: 0,
      monitors: [
        {
          id: 'r0',
          name: 'Mon0',
          widthPx: 3000,
          heightPx: 2000,
          scale: 2,
          mmW: 300,
          mmH: 200,
          posX: 0,
          posY: 0,
          primary: true,
        },
      ],
    })
    resetOffsets()
  }

  return {
    local,
    remote,
    setHost,
    moveHost,
    setHostOffset,
    resetOffsets,
    __dev_mock__,
  }
})

export function hostBounds(host: HostLayout) {
  if (!host.monitors.length) {
    return { minX: 0, minY: 0, maxX: 0, maxY: 0, width: 0, height: 0 }
  }
  let minX = Infinity
  let minY = Infinity
  let maxX = -Infinity
  let maxY = -Infinity
  for (const m of host.monitors) {
    if (m.posX < minX) minX = m.posX
    if (m.posY < minY) minY = m.posY
    if (m.posX + m.widthPx > maxX) maxX = m.posX + m.widthPx
    if (m.posY + m.heightPx > maxY) maxY = m.posY + m.heightPx
  }
  return {
    minX,
    minY,
    maxX,
    maxY,
    width: maxX - minX,
    height: maxY - minY,
  }
}

export function hostOuterRect(host: HostLayout) {
  const b = hostBounds(host)
  return {
    x: host.offsetX + b.minX,
    y: host.offsetY + b.minY,
    width: b.width,
    height: b.height,
  }
}
