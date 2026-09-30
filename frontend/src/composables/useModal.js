/* ===========================================================================
   useModal —— 对话框的焦点与键盘行为
   ---------------------------------------------------------------------------
   解决的问题：全站 5 处 `aria-modal="true"` 但配套行为一个都没有——
   焦点陷阱 0 处、背景未屏蔽。弹窗打开时按 Tab 会走到背后的侧边栏和文件卡片上，
   屏幕阅读器也仍会朗读背景内容。

   统一在这里做四件事：
     1. 打开时把焦点移入对话框（优先 initialFocus，否则第一个可聚焦元素）；
     2. Tab / Shift+Tab 在对话框内循环；
     3. Esc 关闭（走 onClose）；
     4. 关闭时把焦点归还给打开前的元素，并解除背景屏蔽。

   **背景屏蔽用模块级栈，只有栈顶那个对话框生效。**
   这不是洁癖：如果每个实例各自 inert 自己之外的兄弟节点，那么同时打开两个对话框时
   （例如分享弹窗上再叠加一个确认框），A 会把 B 设成 inert、B 也会把 A 设成 inert，
   两个都变得点不动。所以 inert 与 keydown 都必须由栈顶独占，下层弹窗在栈顶关闭后
   重新接管。焦点归还则各实例独立——弹栈后焦点应当回到「上一层弹窗里触发它的那个按钮」。

   屏蔽用 `inert` 而不是 `aria-hidden`：`inert` 同时挡住指针事件、键盘焦点和辅助技术，
   而 `aria-hidden` 只影响辅助技术、键盘仍能 Tab 进去。做法是自内向外逐级给
   **兄弟节点**加 inert，因此对 Teleport 到 body 的对话框（FilePreview）和在组件树内的
   对话框（ConfirmDialog / ShareDialog）同样有效。

   注意：`aria-live` 区域（Toast）不屏蔽——否则对话框开着的时候，
   里头的操作反馈会被一并静音。这与 inert 的常规用法是有意偏离的一处。
   =========================================================================== */

import { watch, nextTick, onBeforeUnmount } from 'vue'

const FOCUSABLE = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',')

/** 栈：每个元素 { container(), onClose, initialFocus(), el }，只有栈顶生效 */
const stack = []
/** 当前被加上 inert 的节点（由栈顶持有） */
let inertNodes = []
let listening = false

/** 可见的可聚焦元素。offsetParent 为 null 说明被 display:none 隐藏，要排除。 */
function focusables(root) {
  return Array.from(root.querySelectorAll(FOCUSABLE)).filter(
    (n) => n.offsetParent !== null
  )
}

function top() {
  return stack[stack.length - 1] || null
}

function clearInert() {
  for (const n of inertNodes) n.removeAttribute('inert')
  inertNodes = []
}

function applyInert(root) {
  clearInert()
  let node = root
  while (node && node.parentElement) {
    for (const sib of Array.from(node.parentElement.children)) {
      if (sib === node || sib.nodeType !== 1) continue
      // 预感区域例外，见文件头说明
      if (sib.matches('[aria-live]')) continue
      if (!sib.hasAttribute('inert')) {
        sib.setAttribute('inert', '')
        inertNodes.push(sib)
      }
    }
    node = node.parentElement
  }
}

/** 栈顶变更后重新指派 inert 与焦点 */
function syncTop() {
  const t = top()
  if (!t) {
    clearInert()
    return
  }
  applyInert(t.el())
}

function onKeydown(e) {
  const t = top()
  if (!t) return
  const root = t.el()
  if (!root) return

  if (e.key === 'Escape') {
    e.preventDefault()
    e.stopPropagation()
    t.onClose?.()
    return
  }
  if (e.key !== 'Tab') return

  const list = focusables(root)
  if (!list.length) {
    // 对话框里没有可聚焦元素：把焦点扣在容器上，别让 Tab 漏到背景
    e.preventDefault()
    root.focus()
    return
  }
  const first = list[0]
  const last = list[list.length - 1]
  const active = document.activeElement

  // 焦点已在框外（例如点了遮罩）：拉回来
  if (!root.contains(active)) {
    e.preventDefault()
    ;(e.shiftKey ? last : first).focus()
    return
  }
  if (e.shiftKey && active === first) {
    e.preventDefault()
    last.focus()
  } else if (!e.shiftKey && active === last) {
    e.preventDefault()
    first.focus()
  }
}

function bind() {
  if (listening) return
  // 捕获阶段监听：先于组件自身的 keydown 处理，避免与页面快捷键打架
  document.addEventListener('keydown', onKeydown, true)
  listening = true
}
function unbind() {
  if (!listening) return
  document.removeEventListener('keydown', onKeydown, true)
  listening = false
}

/**
 * @param {import('vue').Ref<boolean>|Function} isOpen 响应式的开关（ref 或 getter）
 * @param {object}   options
 * @param {object}   options.container    对话框根元素的模板 ref（必须带 tabindex="-1"）
 * @param {Function} options.onClose      Esc 与关闭时调用
 * @param {Function} [options.initialFocus] 返回首选的聚焦目标，返回空则用第一个可聚焦元素
 */
export function useModal(isOpen, options = {}) {
  let restoreTo = null
  let entry = null

  const container = () => {
    const c = options.container
    return c && typeof c === 'object' && 'value' in c ? c.value : c
  }

  function push() {
    restoreTo = document.activeElement
    // el() 延迟到需要时再取，避免模板 ref 尚未挂载
    entry = { el: container, onClose: options.onClose, initialFocus: options.initialFocus }
    stack.push(entry)
    bind()
    syncTop()
    const root = container()
    const target = options.initialFocus?.() || (root && focusables(root)[0]) || root
    target?.focus?.()
  }

  function pop() {
    const i = stack.indexOf(entry)
    if (i >= 0) stack.splice(i, 1)
    entry = null
    syncTop()
    if (!stack.length) unbind()
    // 只在原节点仍在文档中时才归还焦点，否则会 focus 到一个已销毁的元素
    if (restoreTo && document.contains(restoreTo)) restoreTo.focus?.()
    restoreTo = null
  }

  watch(isOpen, async (open) => {
    if (open) {
      await nextTick()
      // 同一 tick 内开关反复切换时，可能已经关掉了
      if (entry) return
      push()
    } else if (entry) {
      pop()
    }
  })

  // 组件在对话框打开时被卸载（如路由切换）：必须出栈，否则整页永久 inert
  onBeforeUnmount(() => {
    if (entry) pop()
  })
}
