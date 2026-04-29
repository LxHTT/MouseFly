<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import MonitorCanvas from '../components/MonitorCanvas.vue'
import { useLayoutStore } from '../stores/layout'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'

const layout = useLayoutStore()
const { t } = useI18n()

const hasLocal = computed(() => layout.local !== null)
const hasRemote = computed(() => layout.remote !== null)

onMounted(() => {
  if (new URLSearchParams(window.location.search).has('mock')) {
    layout.__dev_mock__()
  }
  ;(window as unknown as { __mfMock?: () => void }).__mfMock = () =>
    layout.__dev_mock__()
})

function reset() {
  layout.resetOffsets()
}
</script>

<template>
  <div class="space-y-4 flex flex-col" style="min-height: 480px">
    <Card class="flex-1">
      <CardHeader>
        <div class="flex items-center justify-between">
          <CardTitle>{{ t('layout.title') }}</CardTitle>
          <Button
            size="sm"
            variant="outline"
            :disabled="!hasLocal && !hasRemote"
            @click="reset"
          >
            {{ t('layout.reset') }}
          </Button>
        </div>
      </CardHeader>

      <CardContent class="relative" style="min-height: 420px">
        <MonitorCanvas v-if="hasLocal || hasRemote" />
        <div
          v-else
          class="absolute inset-0 flex items-center justify-center text-sm text-muted-foreground px-6 text-center"
        >
          {{ t('layout.empty') }}
        </div>
      </CardContent>
    </Card>

    <div
      v-if="hasLocal || hasRemote"
      class="flex items-center gap-4 text-xs text-muted-foreground"
    >
      <div class="flex items-center gap-2">
        <Badge variant="default" class="h-3 w-3 p-0 bg-blue-500" />
        {{ t('layout.legendLocal') }}
      </div>
      <div class="flex items-center gap-2">
        <Badge variant="default" class="h-3 w-3 p-0 bg-green-500" />
        {{ t('layout.legendRemote') }}
      </div>
      <span v-if="!hasRemote" class="text-muted-foreground/60">
        {{ t('layout.waitingRemote') }}
      </span>
    </div>
  </div>
</template>
