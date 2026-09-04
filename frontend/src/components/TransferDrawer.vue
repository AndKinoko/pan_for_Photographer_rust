<script setup>
import { computed } from 'vue'
import { useTransfer } from '../composables/useTransfer'
import { formatSize } from '../api'

const {
  state,
  uploadOverall,
  setTab,
  closeDrawer,
  cancelUpload,
  removeUpload,
  cancelDownload,
  clearDone,
} = useTransfer()

const tabs = [
  { key: 'upload', label: '上传列队', icon: '⬆️' },
  { key: 'download', label: '下载列队', icon: '⬇️' },
]

const activeItems = computed(() =>
  state.activeTab === 'upload' ? state.uploads : state.downloads
)

const hasClearable = computed(() =>
  activeItems.value.some(
    (t) => t.status === 'done' || t.status === 'error' || t.status === 'cancelled'
  )
)

const overallText = computed(() => {
  if (state.activeTab === 'upload') {
    return state.uploads.length ? `总进度 ${uploadOverall.value}%` : ''
  }
  return ''
})

function statusIcon(t) {
  if (
    t.status === 'uploading' ||
    t.status === 'downloading' ||
    t.status === 'saving'
  ) {
    return 'spinner sm'
  }
  if (t.status === 'done') return 'ok'
  if (t.status === 'error' || t.status === 'cancelled') return 'bad'
  return ''
}

function statusEmoji(t) {
  if (t.status === 'done') return '✅'
  if (t.status === 'saving') return '💾'
  if (t.status === 'error') return '⚠️'
  if (t.status === 'cancelled') return '✕'
  return ''
}

function isActive(t) {
  return (
    t.status === 'pending' ||
    t.status === 'uploading' ||
    t.status === 'downloading' ||
    t.status === 'saving'
  )
}

function itemSize(t) {
  if (state.activeTab === 'upload') return formatSize(t.size)
  // 下载：显示 "已下载 X / Y MB"（后端给 Content-Length 时）；否则只显示已下载
  if (t.size) {
    return `${formatSize(t.loaded || 0)} / ${formatSize(t.size)}`
  }
  return t.loaded ? formatSize(t.loaded) : ''
}

function cancel(t) {
  if (state.activeTab === 'upload') cancelUpload(t.id)
  else cancelDownload(t.id)
}

function remove(t) {
  removeUpload(t.id)
}

function onClear() {
  clearDone(state.activeTab)
}
</script>

<template>
  <Transition name="td-fade">
    <div v-if="state.drawerOpen" class="td-backdrop" @click="closeDrawer" />
  </Transition>

  <Transition name="td-slide">
    <aside v-if="state.drawerOpen" class="transfer-drawer" aria-label="传输面板">
      <header class="td-head">
        <div>
          <strong class="td-title">传输</strong>
          <div v-if="overallText" class="td-sub muted">{{ overallText }}</div>
        </div>
        <button class="btn-icon btn-ghost" aria-label="关闭" @click="closeDrawer">✕</button>
      </header>

      <div class="td-tabs">
        <button
          v-for="tab in tabs"
          :key="tab.key"
          class="td-tab"
          :class="{ active: state.activeTab === tab.key }"
          @click="setTab(tab.key)"
        >
          {{ tab.icon }} {{ tab.label }}
        </button>
      </div>

      <div class="td-body">
        <div v-if="!activeItems.length" class="td-empty muted">暂无任务</div>
        <ul v-else class="td-list">
          <li v-for="t in activeItems" :key="t.id" class="td-item" :class="t.status">
            <div class="td-row">
              <span class="td-icon" :class="statusIcon(t)">
                <span v-if="statusIcon(t) === 'spinner sm'" class="spinner sm" />
                <template v-else>{{ statusEmoji(t) }}</template>
              </span>
              <div class="td-main">
                <div class="td-name truncate" :title="t.name">
                  {{ t.name }}
                  <span v-if="itemSize(t)" class="muted small">· {{ itemSize(t) }}</span>
                </div>
                <div class="bar-track">
                  <div
                    class="bar-fill"
                    :class="t.status"
                    :style="{ width: (t.progress || 0) + '%' }"
                  />
                </div>
                <div v-if="t.error" class="td-error truncate">{{ t.error }}</div>
                <div v-else-if="isActive(t)" class="td-progress muted small">
                  {{ Math.round(t.progress || 0) }}%
                </div>
              </div>
              <button
                v-if="isActive(t)"
                class="td-action td-action-danger"
                title="取消"
                @click="cancel(t)"
              >
                ✕
              </button>
              <button
                v-else
                class="td-action td-action-danger"
                title="移除"
                @click="remove(t)"
              >
                ✕
              </button>
            </div>
          </li>
        </ul>
      </div>

      <footer class="td-foot">
        <button
          class="btn btn-sm btn-ghost"
          :disabled="!hasClearable"
          @click="onClear"
        >
          清除已完成
        </button>
      </footer>
    </aside>
  </Transition>
</template>

<style scoped>
.td-backdrop {
  position: fixed;
  inset: 0;
  background: var(--bg-overlay);
  z-index: 130;
}
.transfer-drawer {
  position: fixed;
  top: 0;
  bottom: 0;
  left: var(--sidebar-width);
  width: min(360px, 90vw);
  background: var(--bg-elevated);
  border-right: 1px solid var(--border);
  box-shadow: var(--shadow-lg);
  z-index: 140;
  display: flex;
  flex-direction: column;
}
.td-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 16px;
  border-bottom: 1px solid var(--border);
}
.td-title {
  color: var(--text-heading);
  font-size: 1rem;
}
.td-sub {
  font-size: 0.72rem;
  margin-top: 2px;
}
.td-tabs {
  display: flex;
  gap: 6px;
  padding: 10px 12px;
  border-bottom: 1px solid var(--border);
}
.td-tab {
  flex: 1;
  padding: 8px 10px;
  border-radius: var(--radius-sm);
  font-size: 0.86rem;
  font-weight: 600;
  color: var(--text-muted);
  background: transparent;
}
.td-tab.active {
  background: var(--primary-soft);
  color: var(--primary);
}
.td-body {
  flex: 1 1 auto;
  overflow-y: auto;
  padding: 8px 12px;
  min-height: 0;
}
.td-empty {
  padding: 40px 0;
  text-align: center;
  font-size: 0.86rem;
}
.td-list {
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.td-item {
  background: var(--bg-hover);
  border-radius: var(--radius);
  padding: 8px 10px;
}
.td-row {
  display: flex;
  align-items: flex-start;
  gap: 8px;
}
.td-icon {
  flex: 0 0 auto;
  width: 18px;
  display: flex;
  justify-content: center;
  margin-top: 2px;
}
.td-icon.ok,
.td-icon.bad {
  font-size: 0.9rem;
  line-height: 1;
}
.td-main {
  flex: 1 1 auto;
  min-width: 0;
}
.td-name {
  font-size: 0.85rem;
  color: var(--text-heading);
}
.td-action {
  flex: 0 0 auto;
  width: 24px;
  height: 24px;
  border-radius: 6px;
  font-size: 0.8rem;
  color: var(--text-muted);
}
.td-action:hover {
  background: var(--bg-elevated);
  color: var(--text-heading);
}
.td-action.td-action-danger {
  color: var(--danger);
  font-weight: 700;
  font-size: 1rem;
  line-height: 1;
}
.td-action.td-action-danger:hover {
  background: var(--danger);
  color: #fff;
}
.td-error {
  color: var(--danger);
  font-size: 0.75rem;
  margin-top: 2px;
}
.td-progress {
  margin-top: 2px;
}
.td-foot {
  padding: 10px 12px;
  border-top: 1px solid var(--border);
  text-align: right;
}

.bar-track {
  height: 6px;
  background: var(--bg-elevated);
  border-radius: 999px;
  overflow: hidden;
  margin-top: 4px;
}
.bar-fill {
  height: 100%;
  background: var(--primary);
  border-radius: 999px;
  transition: width 0.2s ease;
}
.bar-fill.done {
  background: var(--success);
}
.bar-fill.saving {
  background: var(--primary);
  opacity: 0.7;
}
.bar-fill.error,
.bar-fill.cancelled {
  background: var(--danger);
}
.spinner.sm {
  width: 14px;
  height: 14px;
  border-width: 2px;
}

.td-slide-enter-active,
.td-slide-leave-active {
  transition: transform 0.22s ease;
}
.td-slide-enter-from,
.td-slide-leave-to {
  transform: translateX(-100%);
}
.td-fade-enter-active,
.td-fade-leave-active {
  transition: opacity 0.2s ease;
}
.td-fade-enter-from,
.td-fade-leave-to {
  opacity: 0;
}

@media (max-width: 768px) {
  .transfer-drawer {
    left: 0;
  }
}
</style>