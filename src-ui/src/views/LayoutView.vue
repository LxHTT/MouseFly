<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import MonitorCanvas from '../components/MonitorCanvas.vue'
import { useLayoutStore } from '../stores/layout'
import { Button } from '@/components/ui/button'

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
  if (confirm(t('layout.resetConfirm'))) {
    layout.resetOffsets()
  }
}
</script>

<template>
  <div class="relative w-full h-full">
    <MonitorCanvas v-if="hasLocal || hasRemote" />
    <div
      v-else
      class="absolute inset-0 flex items-center justify-center text-sm text-muted-foreground px-6 text-center"
    >
      {{ t('layout.empty') }}
    </div>
    <div class="absolute top-2 left-2">
      <Button
        size="sm"
        variant="outline"
        class="h-6 text-[10px]"
        :disabled="!hasLocal && !hasRemote"
        @click="reset"
      >
        {{ t('layout.reset') }}
      </Button>
    </div>
  </div>
</template>
