import { defineStore } from 'pinia'
import { ref } from 'vue'
import type { StatusSeverity } from '../ipc'

export type Role = 'sender' | 'receiver' | 'connecting'

export const useLinkStore = defineStore('link', () => {
  const role = ref<Role>('connecting')
  const peer = ref<string>('')
  const inject = ref<boolean>(false)
  const p50us = ref<number>(0)
  const p99us = ref<number>(0)
  const eps = ref<number>(0)
  const offsetNs = ref<number>(0)
  const statusSeverity = ref<StatusSeverity>('info')
  const statusText = ref<string>('Starting…')

  return {
    role,
    peer,
    inject,
    p50us,
    p99us,
    eps,
    offsetNs,
    statusSeverity,
    statusText,
  }
})
