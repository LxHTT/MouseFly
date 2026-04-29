<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useLogStore } from '../stores/log'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'

const { t } = useI18n()
const logStore = useLogStore()
const listEl = ref<HTMLElement | null>(null)
const autoScroll = ref(true)

watch(
  () => logStore.entries.length,
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

const levelVariant: Record<string, 'destructive' | 'default' | 'secondary' | 'outline'> = {
  error: 'destructive',
  warn: 'secondary',
  info: 'default',
  debug: 'outline',
  trace: 'outline',
}
</script>

<template>
  <Card>
    <CardHeader>
      <div class="flex items-center justify-between">
        <CardTitle>{{ t('app.tabs.log') }}</CardTitle>
        <Button size="sm" variant="outline" @click="logStore.clear()">
          {{ t('log.clear') }}
        </Button>
      </div>
    </CardHeader>
    <CardContent>
      <div
        ref="listEl"
        class="h-[480px] overflow-y-auto rounded-lg border bg-muted/30 p-3 space-y-1 font-mono text-xs"
        @scroll="onScroll"
      >
        <div
          v-for="(entry, i) in logStore.entries"
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
        <p v-if="!logStore.entries.length" class="text-center text-muted-foreground py-8">
          {{ t('log.empty') }}
        </p>
      </div>
    </CardContent>
  </Card>
</template>
