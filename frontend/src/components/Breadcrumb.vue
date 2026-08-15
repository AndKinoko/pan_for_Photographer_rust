<script setup>
defineProps({
  /** Array of { id, name }. id null/undefined means root. */
  path: {
    type: Array,
    default: () => [],
  },
})
const emit = defineEmits(['navigate'])
</script>

<template>
  <nav class="breadcrumb" aria-label="路径">
    <button
      class="crumb root"
      :class="{ active: !path.length }"
      @click="emit('navigate', { id: null, name: '全部文件' })"
    >
      🏠 全部文件
    </button>
    <template v-for="(item, i) in path" :key="item.id ?? `p${i}`">
      <span class="sep" aria-hidden="true">/</span>
      <button
        class="crumb"
        :class="{ active: i === path.length - 1 }"
        @click="emit('navigate', item)"
      >
        {{ item.name }}
      </button>
    </template>
  </nav>
</template>

<style scoped>
.breadcrumb {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-wrap: wrap;
  min-height: 44px;
  padding: 4px 0;
}
.crumb {
  padding: 6px 10px;
  border-radius: var(--radius-sm);
  color: var(--text-muted);
  font-size: 0.9rem;
  font-weight: 500;
  min-height: 36px;
  display: inline-flex;
  align-items: center;
  transition: background-color 0.15s ease, color 0.15s ease;
  max-width: 220px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.crumb:hover {
  background: var(--bg-hover);
  color: var(--text-heading);
}
.crumb.active {
  color: var(--primary);
  background: var(--primary-soft);
  pointer-events: none;
}
.root {
  font-weight: 600;
}
.sep {
  color: var(--text-muted);
  user-select: none;
}
</style>
