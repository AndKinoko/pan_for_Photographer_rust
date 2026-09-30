<script setup>
import { computed } from 'vue'
import AppIcon from './AppIcon.vue'
import { useCardMenu } from '../composables/useCardMenu'
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

// ⋯ 菜单的浮层定位与全局监听都在这里——菜单装不进卡片，必须 Teleport 出去，
// 所以它需要自己的坐标计算与滚动关闭策略。详见 useCardMenu.js 的说明。
const {
  open: menuOpen,
  anchorEl,
  menuEl,
  pos: menuPos,
  toggle,
  hide: closeMenu,
} = useCardMenu()

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
  toggle(anchorEl.value, e)
}
function run(action, e) {
  e.stopPropagation()
  closeMenu()
  emit(action, props.item)
}
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
      <AppIcon
        v-else
        class="file-icon"
        :name="isFolder ? 'Folder' : fileIcon(item.file_type, item.name)"
        size="lg"
      />
    </div>

    <div class="info">
      <div class="name truncate" :title="item.name">{{ item.name }}</div>
      <div class="meta truncate" :title="metaText">{{ metaText }}</div>
      <div v-if="context === 'trash'" class="meta muted">
        删除于 {{ dateText }}
      </div>
    </div>

    <div class="menu-wrap">
      <button
        ref="anchorEl"
        class="menu-btn"
        :class="{ open: menuOpen }"
        aria-label="更多操作"
        aria-haspopup="menu"
        :aria-expanded="menuOpen"
        @click="toggleMenu"
      >
        <AppIcon name="MoreHorizontal" size="sm" />
      </button>
      <!-- Teleport 到 body：卡片带 overflow:hidden，菜单留在卡片里会被裁掉左边缘
           和「删除」那一项。定位坐标由 useCardMenu 量锚点算出，走 menuPos。 -->
      <Teleport to="body">
        <Transition name="fade">
          <div
            v-if="menuOpen"
            ref="menuEl"
            class="menu"
            role="menu"
            :style="menuPos"
            @click.stop
          >
            <template v-if="context === 'browse'">
              <button v-if="!isFolder" role="menuitem" @click="run('download', $event)">
                <AppIcon name="Download" size="sm" /> 下载
              </button>
              <button role="menuitem" @click="run('rename', $event)">
                <AppIcon name="Pencil" size="sm" /> 重命名
              </button>
              <button v-if="!isFolder" role="menuitem" @click="run('share', $event)">
                <AppIcon name="Link" size="sm" /> 分享
              </button>
              <button class="danger" role="menuitem" @click="run('remove', $event)">
                <AppIcon name="Trash2" size="sm" /> 删除
              </button>
            </template>
            <template v-else>
              <button role="menuitem" @click="run('restore', $event)">
                <AppIcon name="RotateCcw" size="sm" /> 恢复
              </button>
              <button class="danger" role="menuitem" @click="run('permanent', $event)">
                <AppIcon name="Trash2" size="sm" /> 永久删除
              </button>
            </template>
          </div>
        </Transition>
      </Teleport>
    </div>
  </div>
</template>

<style scoped>
.file-card {
  position: relative;
  /* 离屏卡片跳过布局与绘制。几千张照片时这是主线程压力的主要来源。
     contain-intrinsic-size 带 auto 关键字，浏览器会记住真实高度，
     滚动条不会因为估算值不准而跳动。 */
  content-visibility: auto;
  contain-intrinsic-size: auto 240px;
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
  z-index: var(--z-card-ctrl);
  width: 26px;
  height: 26px;
  border-radius: 6px;
  background: var(--bg-elevated);
  border: 1px solid var(--border-control);
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

/* 图标取代 emoji 后颜色可跟随主题：普通文件用弱化色，不抢缩略图的视觉权重；
   文件夹用品牌色以示区分。 */
.thumb .file-icon {
  color: var(--text-muted);
}
.file-card.folder .thumb {
  background: var(--primary-soft);
}
.file-card.folder .thumb .file-icon {
  color: var(--primary);
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
  z-index: var(--z-card-ctrl);
}
.menu-btn {
  width: 34px;
  height: 34px;
  border-radius: 8px;
  /* 缩略图底色不可预测，用遮罩色保证 ⋯ 在任何照片上都可见 */
  background: var(--bg-overlay);
  color: #fff;
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
  /* 浮层 Teleport 到 body，坐标由 useCardMenu 量锚点算出、走行内样式。
     这里只负责视觉样式，位置交给 top/left: 0 之外的 var(--z-popover) 层级。 */
  position: fixed;
  top: 0;
  left: 0;
  min-width: 150px;
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  box-shadow: var(--shadow-lg);
  padding: 6px;
  display: flex;
  flex-direction: column;
  z-index: var(--z-popover);
}
.menu button {
  text-align: left;
  padding: 6px 10px;
  border-radius: 6px;
  font-size: 0.86rem;
  color: var(--text-heading);
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 34px;
}
.menu button:hover {
  background: var(--bg-hover);
}
.menu button.danger {
  color: var(--danger);
}

/* 触屏设备没有 hover 态。原先选择框与 ⋯ 菜单都靠 :hover 才显形，而 .visible
   又依赖 selectable、selectable 依赖「已有选中项」——形成循环依赖，
   结果是手机上既无法开始多选，也打不开单个文件的操作菜单，整条批量操作链路失效。
   触屏下改为常显。 */
@media (hover: none) {
  .checkbox,
  .menu-btn {
    opacity: 1;
  }
  /* 视觉尺寸保持小巧以免遮住缩略图，但用伪元素把可点区域扩到 44×44
     （WCAG 2.2 的 Web 下限是 24×24，44 是本项目其它控件的既有标准）。 */
  .checkbox::after {
    content: '';
    position: absolute;
    inset: -9px;
  }
  .menu-btn::after {
    content: '';
    position: absolute;
    inset: -5px;
  }
}
</style>
