import { defineStore } from 'pinia'
import { ref } from 'vue'

export interface LogEntry {
  ts: number
  level: 'trace' | 'debug' | 'info' | 'warn' | 'error'
  message: string
}

const MAX_ENTRIES = 500

export const useLogStore = defineStore('log', () => {
  const entries = ref<LogEntry[]>([])

  function push(entry: LogEntry) {
    entries.value.push(entry)
    if (entries.value.length > MAX_ENTRIES) {
      entries.value = entries.value.slice(-MAX_ENTRIES)
    }
  }

  function clear() {
    entries.value = []
  }

  return { entries, push, clear }
})
