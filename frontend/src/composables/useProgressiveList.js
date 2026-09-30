/* ===========================================================================
   useProgressiveList —— 服务端游标分页的状态机
   ---------------------------------------------------------------------------
   名称仍是「渐进加载」，但数据的来源变了：
   2026-09-26 之前它是在**已取回的全量数组**上做切片渲染（前端渐进渲染），
   现在改为按游标向服务端**逐页取数**。后端 `/api/files`、`/api/trash`、
   `/api/search` 已经带上了游标分页，客户端不该再把整个列表拉下来。

   为什么改成服务端分页：切片渲染只解决了「DOM 节点数」，响应体本身仍然
   随库增长 —— 一个几千张照片的目录，首屏就要传几百 KB 的 JSON。
   两者是同一问题的两半，现在两半都补齐了（离屏不渲染由 CSS 的
   `content-visibility` 负责，见 FileCard.vue）。

   ## 为什么是游标而不是页码
   列表按时间倒序，而这个应用的使用场景就是「一边浏览一边有人在上传」。
   用 offset 时新行插到头部会让整个窗口平移，第二页的第一条会与第一页的
   最后一条重复。游标记的是「上一页最后一条的位置」，插入不影响它。

   ## 调用方需要提供什么
   `fetchPage({ limit, cursor })` 返回一个**归一化**过的页对象：
     { files, total, hasMore, nextCursor, folders?, totalFolders?, fileTypes? }
   三个接口的原始字段名有差异（total vs total_files），归一化放在 api.js 里做一次，
   这里就只管分页状态。

   ## 竞态
   搜索是 350ms 防抖 + 筛选项随时可变，因此 `loadFirst` 会递增 `seq`；
   在途的旧请求回来时若 `seq` 已变，结果被丢弃。`reset()` 同样递增 `seq`，
   所以在途请求不会把已经清空的列表又填回去。
   =========================================================================== */

import { ref, computed, onBeforeUnmount } from 'vue'

/** 每页条数。与服务端 `DEFAULT_LIMIT` 保持一致（服务端另有 500 的上限兜底）。 */
export const PAGE_SIZE = 100

/**
 * @param {(params: {limit: number, cursor: string|null}) => Promise<object>} fetchPage
 * @param {{ pageSize?: number }} [options]
 */
export function useProgressiveList(fetchPage, options = {}) {
  const pageSize = options.pageSize || PAGE_SIZE

  const items = ref([]) // 已加载的文件（跨页累积）
  const folders = ref([]) // 文件夹：接口整体返回，不分页（理由见后端 folder_service）
  const fileTypes = ref([])
  const total = ref(0) // 文件总数（不是已加载数）
  const totalFolders = ref(0)
  const nextCursor = ref(null)
  const loadingMore = ref(false)
  const error = ref('')

  // 递增即作废所有在途请求
  let seq = 0

  const hasMore = computed(() => !!nextCursor.value)
  const loaded = computed(() => items.value.length)

  function applyFirst(page) {
    items.value = page.files || []
    total.value = page.total ?? 0
    nextCursor.value = page.hasMore ? page.nextCursor : null
    if (page.folders) {
      folders.value = page.folders
      totalFolders.value = page.totalFolders ?? page.folders.length
    }
    // file_types 只在搜索接口上有；每页都返回，但只有变化时才需要覆盖
    if (page.fileTypes) fileTypes.value = page.fileTypes
  }

  function applyMore(page) {
    const rows = page.files || []
    if (!rows.length) {
      nextCursor.value = null
      return
    }
    // 游标分页在理论上不会重复，但真出现重复时，v-for 的 key 冲突会让
    // 整个列表渲染报错。一行去重的成本远低于那种失败。
    const seen = new Set(items.value.map((f) => f.id))
    items.value = items.value.concat(rows.filter((f) => !seen.has(f.id)))
    // 服务端每次都返回 total；追加时用它覆盖，比本地累加更可信
    total.value = page.total ?? total.value
    nextCursor.value = page.hasMore ? page.nextCursor : null
  }

  /** 加载第一页（也就是「重新加载列表」） */
  async function loadFirst() {
    const mine = ++seq
    error.value = ''
    try {
      const page = await fetchPage({ limit: pageSize, cursor: null })
      if (mine !== seq) return
      applyFirst(page)
    } catch (e) {
      if (mine !== seq) return
      error.value = e.message || '加载失败'
      items.value = []
      folders.value = []
      total.value = 0
      nextCursor.value = null
    }
  }

  /** 追加下一页 */
  async function loadMore() {
    const cursor = nextCursor.value
    if (!cursor || loadingMore.value) return
    const mine = seq
    loadingMore.value = true
    error.value = ''
    try {
      const page = await fetchPage({ limit: pageSize, cursor })
      // 期间若列表被重置或重新搜索过，这一页属于上一个查询，丢弃
      if (mine !== seq) return
      applyMore(page)
    } catch (e) {
      if (mine !== seq) return
      error.value = e.message || '加载更多失败'
    } finally {
      if (mine === seq) loadingMore.value = false
    }
  }

  /** 清空并作废在途请求。切换目录/关键词/筛选项时调用。 */
  function reset() {
    seq++
    items.value = []
    folders.value = []
    fileTypes.value = []
    total.value = 0
    totalFolders.value = 0
    nextCursor.value = null
    loadingMore.value = false
    error.value = ''
  }

  /** 文件夹由另一个接口提供时（Home 的 /api/folders），用它写入。 */
  function setFolders(list, count) {
    folders.value = list || []
    totalFolders.value = count ?? folders.value.length
  }

  /** 本地移除一个文件（删除/移入回收站后），省掉重拉整页。 */
  function removeFile(id) {
    const before = items.value.length
    items.value = items.value.filter((f) => f.id !== id)
    if (items.value.length !== before) total.value = Math.max(0, total.value - 1)
  }

  /** 本地移除一个文件夹。 */
  function removeFolder(id) {
    const before = folders.value.length
    folders.value = folders.value.filter((f) => f.id !== id)
    if (folders.value.length !== before) {
      totalFolders.value = Math.max(0, totalFolders.value - 1)
    }
  }

  // 组件卸载时作废在途请求，避免往已卸载的组件状态里写
  onBeforeUnmount(() => {
    seq++
  })

  return {
    items,
    folders,
    fileTypes,
    total,
    totalFolders,
    loaded,
    hasMore,
    nextCursor,
    loadingMore,
    error,
    loadFirst,
    loadMore,
    reset,
    setFolders,
    removeFile,
    removeFolder,
  }
}
