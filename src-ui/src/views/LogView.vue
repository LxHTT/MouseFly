<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useLogStore } from '../stores/log'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { Download, Search, Filter } from 'lucide-vue-next'

const { t } = useI18n()
const logStore = useLogStore()
const listEl = ref<HTMLElement | null>(null)
const autoScroll = ref(true)
const searchQuery = ref('')
const levelFilter = ref<Set<string>>(new Set(['error', 'warn', 'info', 'debug', 'trace']))

const filteredEntries = computed(() => {
  let entries = logStore.entries
  if (searchQuery.value) {
    const q = searchQuery.value.toLowerCase()
    entries = entries.filter((e) => e.message.toLowerCase().includes(q))
  }
  entries = entries.filter((e) => levelFilter.value.has(e.level))
  return entries
})

const levelCounts = computed(() => {
  const counts: Record<string, number> = { error: 0, warn: 0, info: 0, debug: 0, trace: 0 }
  for (const e of logStore.entries) {
    if (counts[e.level] !== undefined) counts[e.level]++
  }
  return counts
})

watch(
  () => filteredEntries.value.length,
  async () => {
    if (!autoScroll.value) return
    await nextTick()
    if (listEl.value) listEl.value.scrollTop = listEl.value.scrollHeight
  },
)

function onScroll() {
  if (!listEl.value) return
  const { scrollTop, scrollHeight, clientHeight } = listEl.value
  autoScroll.value = scrollHeight - scrollTop - clientHeight < 40
}

function toggleLevel(level: string) {
  if (levelFilter.value.has(level)) {
    levelFilter.value.delete(level)
  } else {
    levelFilter.value.add(level)
  }
  levelFilter.value = new Set(levelFilter.value)
}

function exportLogs() {
  const lines = filteredEntries.value.map((e) => {
    const ts = new Date(e.ts).toISOString()
    return `${ts} [${e.level.toUpperCase()}] ${e.message}`
  })
  const blob = new Blob([lines.join('\n')], { type: 'text/plain' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = `mousefly-logs-${Date.now()}.txt`
  a.click()
  URL.revokeObjectURL(url)
}

const levelVariant: Record<string, 'destructive' | 'default' | 'secondary' | 'outline'> = {
  error: 'destructive',
  warn: 'secondary',
  info: 'default',
  debug: 'outline',
  trace: 'outline',
}
</script>

<template>
  <div class="flex flex-col h-full p-3 space-y-2">
    <div class="flex items-center justify-between">
      <div class="relative flex-1 mr-2">
        <Search class="absolute left-2 top-1/2 -translate-y-1/2 h-3 w-3 text-muted-foreground" />
        <Input
          v-model="searchQuery"
          :placeholder="t('log.searchPlaceholder')"
          class="h-7 pl-7 text-xs"
        />
      </div>
      <div class="flex gap-1">
        <Button size="sm" variant="outline" class="h-6 text-[10px]" @click="exportLogs">
          <Download class="h-3 w-3 mr-1" />
          {{ t('log.export') }}
        </Button>
        <Button size="sm" variant="outline" class="h-6 text-[10px]" @click="logStore.clear()">
          {{ t('log.clear') }}
        </Button>
      </div>
    </div>
    <div class="flex items-center gap-1 flex-wrap">
      <Filter class="h-3 w-3 text-muted-foreground" />
      <Badge
        v-for="level in ['error', 'warn', 'info', 'debug', 'trace']"
        :key="level"
        :variant="levelFilter.has(level) ? levelVariant[level] : 'outline'"
        class="cursor-pointer text-[10px] h-5 opacity-70 hover:opacity-100 transition-opacity"
        @click="toggleLevel(level)"
      >
        {{ level }} ({{ levelCounts[level] }})
      </Badge>
    </div>
    <div
      ref="listEl"
      class="flex-1 overflow-y-auto rounded-lg border bg-muted/30 p-3 space-y-1 font-mono text-xs select-text"
      @scroll="onScroll"
    >
      <div
        v-for="(entry, i) in filteredEntries"
        :key="i"
        class="flex gap-3 items-start"
      >
        <span class="shrink-0 text-muted-foreground text-[10px] w-16 text-right">
          {{ new Date(entry.ts).toLocaleTimeString([], { hour12: false }) }}
        </span>
        <Badge :variant="levelVariant[entry.level] ?? 'outline'" class="shrink-0 w-14 justify-center text-[10px]">
          {{ entry.level }}
        </Badge>
        <span class="flex-1 break-all">{{ entry.message }}</span>
      </div>
      <p v-if="!filteredEntries.length && logStore.entries.length" class="text-center text-muted-foreground py-8 text-xs">
        {{ t('log.noMatch') }}
      </p>
      <p v-else-if="!logStore.entries.length" class="text-center text-muted-foreground py-8 text-xs">
        {{ t('log.empty') }}
      </p>
    </div>
  </div>
</template>
