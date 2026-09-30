/* ===========================================================================
   useCardMenu —— 卡片 ⋯ 菜单的浮层定位与生命周期
   ---------------------------------------------------------------------------
   解决的问题：菜单装不进卡片，原地渲染必然被裁。

   把数摆出来（网格见 Home.vue / Search.vue）：
     · 最窄的卡片 —— 宽度 ≤768px 时网格是 minmax(140px, 1fr)，最窄到 140px；
       缩略图 4:3 占 105px，加信息区约 62px，卡片总高约 169px。
     · 「下载 / 重命名 / 分享 / 删除」四项菜单 —— 4×34 行高 + 12 内边距 + 2 边框
       = 150px 高、150px 宽（min-width）。
   140×169 的卡片里放一个 150×150 的菜单，横向差 16px、纵向差 25px，
   而 .file-card 带 overflow:hidden（它本来只是为了让缩略图贴住圆角），
   于是左边缘和「删除」那一项被整齐地切掉。所以菜单必须脱离卡片。

   做法：Teleport 到 body + position: fixed，彻底绕开卡片的裁剪与层叠上下文。
   坐标由锚点（卡片右上角的 ⋯ 按钮）量一次 getBoundingClientRect 得到，
   翻转规则只有「右放不下就左移」「下放不下就上翻」两条，因此不引入 floating-ui
   这类依赖——它的价值在于任意锚点、任意策略，这里用不上。

   两个必须由本模块承担、而不是可选的职责：
     1. **全局监听按需挂载**。原先 FileCard 在 onMounted 就永久挂一个 document
        click 监听器：一个含 2000 张照片的目录就是 2000 个监听器，此后每一次点击
        都要跑 2000 个回调。改成只在菜单打开期间挂载后，同一时刻最多只有 1 个。
     2. **滚动时关闭**。浮层是 fixed，卡片滚走了它不会跟；留在原地会盖住别的卡片，
        而此时点「删除」作用在你已经看不见的那个文件上——对一个破坏性操作来说，
        上下文丢失比菜单消失糟糕得多。所以滚动与缩放一律关闭。
   =========================================================================== */

import { ref, nextTick, onBeforeUnmount } from 'vue'

/** 浮层与视口边缘的最小留白 */
const VIEWPORT_MARGIN = 8
/** 浮层与 ⋯ 按钮之间的间隙 */
const ANCHOR_GAP = 6

export function useCardMenu() {
  const open = ref(false)
  /** ⋯ 按钮，作为定位锚点 */
  const anchorEl = ref(null)
  /** 浮层本体 */
  const menuEl = ref(null)
  /** 算好的 fixed 坐标，交给模板的行内样式 */
  const pos = ref({ top: '0px', left: '0px' })

  /**
   * 量出浮层应该待在哪儿。
   *
   * 坐标为整数：fixed 定位落在半像素上时，菜单里 0.86rem 的文字会被重采样，
   * 笔画发虚——这是「对齐像素」最容易被忽略的一处。
   */
  function place() {
    const anchor = anchorEl.value
    const menu = menuEl.value
    if (!anchor || !menu) return

    const a = anchor.getBoundingClientRect()
    const mw = menu.offsetWidth
    const mh = menu.offsetHeight
    const vw = document.documentElement.clientWidth
    const vh = document.documentElement.clientHeight

    // 水平：右边缘与 ⋯ 按钮右对齐，视觉上是「挂在按钮下面」。
    // 右边放不下就退到按钮左边缘，再越界就贴住视口边缘。
    let left = a.right - mw
    if (left < VIEWPORT_MARGIN) left = a.left
    left = Math.min(left, vw - mw - VIEWPORT_MARGIN)
    left = Math.max(left, VIEWPORT_MARGIN)

    // 垂直：默认向下挂在按钮下方；下方装不下、且上方比下方更宽裕时翻向上。
    // 比较「上方可用」与「下方可用」而不是无脑上翻，是为了让菜单尽可能朝
    // 空间大的一侧展开——卡片本来就矮，下方常常只差几个像素。
    let top = a.bottom + ANCHOR_GAP
    const below = vh - VIEWPORT_MARGIN - top
    if (mh > below) {
      const above = a.top - ANCHOR_GAP - VIEWPORT_MARGIN
      if (above > below) top = a.top - ANCHOR_GAP - mh
    }
    top = Math.min(top, vh - mh - VIEWPORT_MARGIN)
    top = Math.max(top, VIEWPORT_MARGIN)

    pos.value = { top: `${Math.round(top)}px`, left: `${Math.round(left)}px` }
  }

  function onDocClick(e) {
    // 浮层内部已由 @click.stop 拦住，这里只是兜底（例如将来去掉那个修饰符）。
    if (menuEl.value && menuEl.value.contains(e.target)) return
    hide()
  }

  function attach() {
    document.addEventListener('click', onDocClick)
    // capture 让内层滚动容器（缩略图横向列表、侧边栏等）的滚动也一并接住。
    window.addEventListener('scroll', hide, { passive: true, capture: true })
    window.addEventListener('resize', hide)
  }

  function detach() {
    document.removeEventListener('click', onDocClick)
    window.removeEventListener('scroll', hide, { capture: true })
    window.removeEventListener('resize', hide)
  }

  function hide() {
    if (!open.value) return
    open.value = false
    detach()
  }

  async function show(anchor) {
    anchorEl.value = anchor
    open.value = true
    attach()
    // 先等浮层进 DOM 才能量到它自己的宽高。nextTick 是微任务，
    // 浏览器要到下一个绘制帧才会画，所以量完再定位不会被看见
    // ——何况 fade 过渡的 enter-from 本身就是 opacity: 0。
    await nextTick()
    place()
  }

  /**
   * @param {HTMLElement} anchor ⋯ 按钮
   * @param {Event} [e] 传入则就地阻止冒泡，避免这次点击立刻被 onDocClick 当成「点在外面」
   */
  function toggle(anchor, e) {
    if (e) e.stopPropagation()
    if (open.value) hide()
    else show(anchor)
  }

  onBeforeUnmount(detach)

  return { open, anchorEl, menuEl, pos, toggle, hide, place }
}
