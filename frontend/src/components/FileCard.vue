<script setup>
import { ref, computed, onMounted, onBeforeUnmount } from 'vue'
import {
  authUrl,
  fileIcon,
  formatSize,
  formatDate,
  isImageFile,
} from '../api'

const props = defineProps({
  item: { type: Object, required: true },
  kind: { type: String, default: 'file' }, // 'file' | 'folder'
  selected: { type: Boolean, default: false },
  selectable: { type: Boolean, default: false },
  context: { type: String, default: 'browse' }, // 'browse' | 'trash'
})

const emit = defineEmits([
  'click',
  'toggle-select',
  'rename',
  'remove',
  'share',
  'download',
  'restore',
  'permanent',
])

const menuOpen = ref(false)
const menuEl = ref(null)

const isFolder = computed(() => props.kind === 'folder')
const isImg = computed(() =>
  !isFolder.value && isImageFile(props.item.file_type, props.item.name)
)
const thumbUrl = computed(() => {
  if (isFolder.value) return null
  return props.item.thumb_url || props.item.preview_url || null
})
const metaText = computed(() => {
  if (isFolder.value) {
    const hasCounts =
      props.item.file_count != null || props.item.subfolder_count != null
    if (hasCounts) {
      const f = props.item.file_count ?? 0
      const s = props.item.subfolder_count ?? 0
      return `${f} 个文件 · ${s} 个子文件夹`
    }
    // Fallback for raw folder objects (e.g. search results) without counts.
    return formatDate(
      props.item.created_at || props.item.updated_at
    )
  }
  return props.item.formatted_size || formatSize(props.item.size)
})
const dateText = computed(() => {
  const v =
    props.item.uploaded_at || props.item.created_at || props.item.updated_at
  return formatDate(v)
})

function onCardClick() {
  if (props.selectable) {
    emit('toggle-select', props.item)
  } else {
    emit('click', props.item)
  }
}
function onCheck(e) {
  e.stopPropagation()
  emit('toggle-select', props.item)
}
function toggleMenu(e) {
  e.stopPropagation()
  menuOpen.value = !menuOpen.value
}
function run(action, e) {
  e.stopPropagation()
  menuOpen.value = false
  emit(action, props.item)
}
function onDocClick(e) {
  if (menuEl.value && !menuEl.value.contains(e.target)) {
    menuOpen.value = false
  }
}
onMounted(() => document.addEventListener('click', onDocClick))
onBeforeUnmount(() => document.removeEventListener('click', onDocClick))
</script>

<template>
  <div
    class="file-card"
    :class="{ selected, folder: isFolder, selectable }"
    tabindex="0"
    role="button"
    @click="onCardClick"
    @keydown.enter.prevent="onCardClick"
  >
    <label
      v-if="context === 'browse'"
      class="checkbox"
      :class="{ visible: selectable || selected }"
      :title="selected ? '取消选择' : '选择'"
      @click="onCheck"
    >
      <input type="checkbox" :checked="selected" />
    </label>

    <div class="thumb">
      <img
        v-if="isImg && thumbUrl"
        :src="authUrl(thumbUrl)"
        loading="lazy"
        alt=""
        @error="$event.target.style.display = 'none'"
      />
      <span v-else class="emoji">{{ isFolder ? '📁' : fileIcon(item.file_type, item.name) }}</span>
    </div>

    <div class="info">
      <div class="name truncate" :title="item.name">{{ item.name }}</div>
      <div class="meta truncate">{{ metaText }}</div>
      <div v-if="context === 'trash'" class="meta muted">
        删除于 {{ dateText }}
      </div>
    </div>

    <div ref="menuEl" class="menu-wrap">
      <button
        class="menu-btn"
        :class="{ open: menuOpen }"
        aria-label="更多操作"
        @click="toggleMenu"
      >
        ⋯
      </button>
      <Transition name="fade">
        <div v-if="menuOpen" class="menu" role="menu" @click.stop>
          <template v-if="context === 'browse'">
            <button v-if="!isFolder" role="menuitem" @click="run('download', $event)">
              ⬇️ 下载
            </button>
            <button role="menuitem" @click="run('rename', $event)">✏️ 重命名</button>
            <button v-if="!isFolder" role="menuitem" @click="run('share', $event)">
              🔗 分享
            </button>
            <button class="danger" role="menuitem" @click="run('remove', $event)">
              🗑️ 删除
            </button>
          </template>
          <template v-else>
            <button role="menuitem" @click="run('restore', $event)">♻️ 恢复</button>
            <button class="danger" role="menuitem" @click="run('permanent', $event)">
              ⨯ 永久删除
            </button>
          </template>
        </div>
      </Transition>
    </div>
  </div>
</template>

<style scoped>
.file-card {
  position: relative;
  display: flex;
  flex-direction: column;
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  overflow: hidden;
  cursor: pointer;
  transition: border-color 0.16s ease, box-shadow 0.16s ease,
    transform 0.06s ease;
  outline: none;
}
.file-card:hover {
  border-color: var(--border-strong);
  box-shadow: var(--shadow);
}
.file-card:focus-visible {
  border-color: var(--primary);
  box-shadow: 0 0 0 3px var(--primary-soft);
}
.file-card.selected {
  border-color: var(--primary);
  box-shadow: 0 0 0 2px var(--primary);
}

.checkbox {
  position: absolute;
  top: 8px;
  left: 8px;
  z-index: 2;
  width: 26px;
  height: 26px;
  border-radius: 6px;
  background: var(--bg-elevated);
  border: 1px solid var(--border-strong);
  display: flex;
  align-items: center;
  justify-content: center;
  opacity: 0;
  transition: opacity 0.15s ease;
  cursor: pointer;
}
.file-card:hover .checkbox,
.checkbox.visible {
  opacity: 1;
}
.checkbox input {
  margin: 0;
  width: 16px;
  height: 16px;
  accent-color: var(--primary);
  cursor: pointer;
}

.thumb {
  width: 100%;
  aspect-ratio: 4 / 3;
  background: var(--bg-hover);
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
}
.thumb img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.thumb .emoji {
  font-size: 2.6rem;
  opacity: 0.9;
}
.file-card.folder .thumb {
  background: var(--primary-soft);
}
.file-card.folder .thumb .emoji {
  font-size: 3rem;
}

.info {
  padding: 10px 12px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.name {
  font-size: 0.92rem;
  font-weight: 600;
  color: var(--text-heading);
}
.meta {
  font-size: 0.78rem;
  color: var(--text-muted);
}
.muted {
  color: var(--text-muted);
}

.menu-wrap {
  position: absolute;
  top: 6px;
  right: 6px;
  z-index: 3;
}
.menu-btn {
  width: 34px;
  height: 34px;
  border-radius: 8px;
  background: var(--bg-overlay);
  color: #fff;
  font-size: 1.1rem;
  font-weight: 700;
  line-height: 1;
  opacity: 0;
  transition: opacity 0.15s ease, background-color 0.15s ease;
  display: flex;
  align-items: center;
  justify-content: center;
}
.file-card:hover .menu-btn,
.menu-btn.open {
  opacity: 1;
}
.menu-btn:hover,
.menu-btn.open {
  background: var(--bg-elevated);
  color: var(--text-heading);
  box-shadow: var(--shadow-sm);
}
.menu {
  position: absolute;
  top: 38px;
  right: 0;
  min-width: 150px;
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  box-shadow: var(--shadow-lg);
  padding: 6px;
  display: flex;
  flex-direction: column;
  z-index: 50;
}
.menu button {
  text-align: left;
  padding: 9px 12px;
  border-radius: 6px;
  font-size: 0.86rem;
  color: var(--text-heading);
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 38px;
}
.menu button:hover {
  background: var(--bg-hover);
}
.menu button.danger {
  color: var(--danger);
}
</style>
