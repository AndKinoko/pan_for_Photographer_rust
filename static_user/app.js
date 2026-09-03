'use strict'

/* ===========================================================================
   普通用户前端 (端口 8001) — 登录 / 浏览 / 预览 / 下载
   =========================================================================== */

const TOKEN_KEY = 'token'
const $ = (id) => document.getElementById(id)

const els = {
  login: $('login'),
  loginForm: $('login-form'),
  username: $('username'),
  password: $('password'),
  loginErr: $('login-err'),
  appView: $('app-view'),
  userName: $('user-name'),
  breadcrumb: $('breadcrumb'),
  loading: $('loading'),
  empty: $('empty'),
  error: $('error'),
  errorMsg: $('error-msg'),
  retryBtn: $('retry-btn'),
  folders: $('folders'),
  files: $('files'),
  folderSection: $('folder-section'),
  fileSection: $('file-section'),
  lightbox: $('lightbox'),
  lbName: $('lb-name'),
  lbBody: $('lb-body'),
  lbDownload: $('lb-download'),
  lbPrev: $('lb-prev'),
  lbNext: $('lb-next'),
  toast: $('toast'),
}

let state = {
  user: null,
  crumbs: [],       // [{ id, name }] ; empty = root
  folders: [],
  files: [],
  selected: new Set(),
  lbIndex: 0,
  toastTimer: null,
}

/* ----------------------------- helpers ----------------------------- */
function authUrl(url) {
  if (!url) return url
  const token = localStorage.getItem(TOKEN_KEY)
  if (!token) return url
  return url + (url.includes('?') ? '&' : '?') + 'token=' + encodeURIComponent(token)
}

/* 通过 Authorization 头拉取媒体并生成 blob 对象URL（更可靠的预览鉴权方式） */
function authBlob(url) {
  return fetch(url, { headers: { Authorization: 'Bearer ' + localStorage.getItem(TOKEN_KEY) } })
    .then(function (res) {
      if (!res.ok) throw new Error('HTTP ' + res.status)
      return res.blob()
    })
    .then(function (b) { return URL.createObjectURL(b) })
}

async function api(path, options = {}) {
  const headers = options.headers || {}
  const token = localStorage.getItem(TOKEN_KEY)
  if (token) headers.Authorization = 'Bearer ' + token
  if (options.body && typeof options.body !== 'string' && !(options.body instanceof FormData)) {
    headers['Content-Type'] = 'application/json'
    options.body = JSON.stringify(options.body)
  }
  try {
    const res = await fetch(path, { ...options, headers })
    let data = null
    try { data = await res.json() } catch { /* non-json */ }
    if (!res.ok || (data && data.success === false)) {
      const msg = (data && (data.error || data.message)) || '请求失败 (' + res.status + ')'
      if (res.status === 401) { showLogin(); }
      const e = new Error(msg)
      e.status = res.status
      throw e
    }
    return data ? data.data : null
  } catch (err) {
    if (err.status) throw err
    throw new Error('网络错误，请稍后重试')
  }
}

function extOf(name = '') {
  const i = String(name).lastIndexOf('.')
  return i >= 0 ? name.slice(i + 1).toLowerCase() : ''
}
function fileIcon(name) {
  const ext = extOf(name)
  const img = ['jpg','jpeg','png','gif','bmp','webp','tiff','tif','nef','cr2','cr3','arw','dng','raf','orf','rw2']
  const vid = ['mp4','mov','avi','mkv','webm','m4v']
  const aud = ['mp3','wav','flac','ogg','aac','m4a']
  if (img.includes(ext)) return '🖼️'
  if (vid.includes(ext)) return '🎬'
  if (aud.includes(ext)) return '🎵'
  if (ext === 'pdf') return '📕'
  if (['doc','docx'].includes(ext)) return '📘'
  if (['xls','xlsx','csv'].includes(ext)) return '📊'
  if (['ppt','pptx'].includes(ext)) return '📙'
  if (['zip','rar','7z','tar','gz'].includes(ext)) return '📦'
  return '📄'
}
function isPreviewable(name) {
  const ext = extOf(name)
  return /^(jpg|jpeg|png|gif|bmp|webp|tiff|tif|nef|cr2|cr3|crw|arw|sr2|srf|dng|raf|orf|rw2|nrw|mp4|mov|webm|m4v|mp3|wav|flac|ogg|pdf)$/.test(ext)
}
function isImage(name) {
  const ext = extOf(name)
  return /^(jpg|jpeg|png|gif|bmp|webp|tiff|tif|nef|cr2|cr3|crw|arw|sr2|srf|dng|raf|orf|rw2|nrw)$/.test(ext)
}
function formatSize(bytes) {
  const n = Number(bytes) || 0
  if (n < 1024) return n + ' B'
  if (n < 1048576) return (n / 1024).toFixed(1) + ' KB'
  if (n < 1073741824) return (n / 1048576).toFixed(1) + ' MB'
  return (n / 1073741824).toFixed(2) + ' GB'
}

function toast(msg) {
  els.toast.textContent = msg
  els.toast.classList.remove('hidden')
  clearTimeout(state.toastTimer)
  state.toastTimer = setTimeout(() => els.toast.classList.add('hidden'), 2600)
}

/* ----------------------------- API ----------------------------- */
async function login(username, password) {
  const data = await api('/api/auth/login', { method: 'POST', body: { username, password } })
  localStorage.setItem(TOKEN_KEY, data.token)
  state.user = data.user
}
async function fetchMe() {
  return api('/api/auth/me')
}
async function listFolders(parentId) {
  const params = parentId != null ? 'parent_id=' + parentId : ''
  return api('/api/folders' + (params ? '?' + params : ''))
}
async function listFiles(folderId) {
  const params = folderId != null ? 'folder_id=' + folderId : ''
  return api('/api/files' + (params ? '?' + params : ''))
}

/* ----------------------------- views ----------------------------- */
function showLogin() {
  localStorage.removeItem(TOKEN_KEY)
  document.body.dataset.view = 'boot'
  els.login.classList.remove('hidden')
  els.appView.classList.add('hidden')
}
function showApp() {
  document.body.dataset.view = ''
  els.login.classList.add('hidden')
  els.appView.classList.remove('hidden')
  els.userName.textContent = (state.user && state.user.username) || ''
  loadCurrent()
}

function currentFolderId() {
  return state.crumbs.length ? state.crumbs[state.crumbs.length - 1].id : null
}

function renderBreadcrumb() {
  els.breadcrumb.innerHTML = ''
  const root = document.createElement('button')
  root.textContent = '🏠 根目录'
  root.className = state.crumbs.length ? '' : 'active'
  root.onclick = () => navigateTo(null)
  els.breadcrumb.appendChild(root)
  let idx = 0
  for (const c of state.crumbs) {
    const sep = document.createElement('span')
    sep.className = 'sep'
    sep.textContent = '/'
    const btn = document.createElement('button')
    btn.textContent = c.name
    btn.className = idx === state.crumbs.length - 1 ? 'active' : ''
    btn.onclick = () => navigateTo(c)
    els.breadcrumb.appendChild(sep)
    els.breadcrumb.appendChild(btn)
    idx++
  }
}

function renderGrids() {
  // 切换文件夹后，清理不在当前列表中的勾选状态
  const currentIds = new Set(state.files.map(function (f) { return f.id }))
  for (const id of Array.from(state.selected)) {
    if (!currentIds.has(id)) state.selected.delete(id)
  }
  els.folders.innerHTML = ''
  els.folderSection.classList.toggle('hidden', state.folders.length === 0)
  for (const f of state.folders) {
    els.folders.appendChild(folderCard(f))
  }
  els.files.innerHTML = ''
  els.fileSection.classList.toggle('hidden', state.files.length === 0)
  state.files.forEach((f, i) => els.files.appendChild(fileCard(f, i)))
  els.empty.classList.toggle('hidden', state.folders.length + state.files.length !== 0)
  updateBatchUI()
}

function folderCard(f) {
  const card = document.createElement('div')
  card.className = 'item'
  card.innerHTML =
    '<div class="thumb-ph">📁</div>' +
    '<div class="meta"><div class="name" title="' + esc(f.name) + '">' + esc(f.name) + '</div>' +
    '<div class="sub">文件夹</div></div>'
  card.onclick = () => enterFolder(f)
  return card
}

function fileCard(f, i) {
  const card = document.createElement('div')
  card.className = 'item'
  const img = isImage(f.name)
  const sel = state.selected.has(f.id)
  const src = img && f.thumb_url ? authUrl(f.thumb_url) : ''
  card.innerHTML =
    '<input class="item-chk" type="checkbox" data-id="' + f.id + '" aria-label="选择" ' + (sel ? 'checked' : '') + ' />' +
    (img && src
      ? '<img class="thumb" loading="lazy" alt="" src="' + src + '" onerror="this.style.display=\'none\';this.nextElementSibling.style.display=\'flex\'"><div class="thumb-ph" style="display:none">' + fileIcon(f.name) + '</div>'
      : '<div class="thumb-ph">' + fileIcon(f.name) + '</div>') +
    '<div class="meta"><div class="name" title="' + esc(f.name) + '">' + esc(f.name) + '</div>' +
    '<div class="sub">' + formatSize(f.size) + '</div></div>'
  card.onclick = function (e) {
    if (e.target.closest('.item-chk')) { toggleSel(f.id); return }
    openLightbox(i)
  }
  return card
}

/* ---------------- 批量下载 ---------------- */
function updateBatchUI() {
  const btn = document.getElementById('batch-dl')
  const n = state.selected.size
  if (btn) { btn.disabled = n === 0; btn.textContent = '⬇ 批量下载(' + n + ')' }
  const all = document.getElementById('sel-all')
  if (all) all.checked = state.files.length > 0 && state.selected.size === state.files.length
}
function toggleSel(id) {
  if (state.selected.has(id)) state.selected.delete(id)
  else state.selected.add(id)
  renderGrids()
  updateBatchUI()
}
function selectedFiles() {
  return state.files.filter(function (f) { return state.selected.has(f.id) })
}
function downloadOne(f) {
  return authBlob(f.download_url).then(function (url) {
    const a = document.createElement('a')
    a.href = url
    a.download = f.original_name || f.name
    document.body.appendChild(a)
    a.click()
    a.remove()
    setTimeout(function () { URL.revokeObjectURL(url) }, 5000)
  })
}
async function batchDownload() {
  const files = selectedFiles()
  if (!files.length) return
  const btn = document.getElementById('batch-dl')
  if (btn) btn.disabled = true
  toast('开始下载 ' + files.length + ' 个文件…')
  let ok = 0
  for (const f of files) {
    try { await downloadOne(f); ok++ }
    catch (err) { /* 单项失败则跳过并继续 */ }
    await new Promise(function (r) { setTimeout(r, 250) }) // 间隔，避免浏览器拦截连续下载
  }
  toast('批量下载完成：成功 ' + ok + ' / ' + files.length)
  if (btn) btn.disabled = false
}

async function loadCurrent() {
  renderBreadcrumb()
  els.loading.classList.remove('hidden')
  els.error.classList.add('hidden')
  els.empty.classList.add('hidden')
  try {
    const id = currentFolderId()
    const [foldersRes, filesRes] = await Promise.all([listFolders(id), listFiles(id)])
    state.folders = (foldersRes && foldersRes.folders) || []
    state.files = filesRes || []
    renderGrids()
  } catch (err) {
    els.errorMsg.textContent = err.message
    els.error.classList.remove('hidden')
  } finally {
    els.loading.classList.add('hidden')
  }
}

function enterFolder(f) {
  state.crumbs.push({ id: f.id, name: f.name })
  loadCurrent()
}
function navigateTo(item) {
  if (!item || item.id == null) { state.crumbs = []; loadCurrent(); return }
  const idx = state.crumbs.findIndex((c) => c.id === item.id)
  state.crumbs = state.crumbs.slice(0, idx + 1)
  loadCurrent()
}

/* ----------------------------- lightbox ----------------------------- */
function openLightbox(i) {
  state.lbIndex = i
  refreshLightbox()
  els.lightbox.classList.remove('hidden')
}
function closeLightbox() {
  els.lightbox.classList.add('hidden')
  els.lbBody.innerHTML = ''
}
function refreshLightbox() {
  const list = state.files
  if (!list.length) return closeLightbox()
  const f = list[Math.max(0, Math.min(state.lbIndex, list.length - 1))]
  state.lbIndex = list.indexOf(f)
  els.lbName.textContent = f.name
  els.lbPrev.classList.toggle('hidden', list.length <= 1)
  els.lbNext.classList.toggle('hidden', list.length <= 1)
  els.lbBody.innerHTML = renderPreview(f)
  hydratePreview(f)
}

/* 先用占位，再用鉴权 fetch 拉取媒体，解码为 blob 对象 URL 后渲染 */
function hydratePreview(f) {
  const node = els.lbBody.querySelector('[data-preview-src]')
  if (!node) return
  const kind = node.getAttribute('data-preview-src')
  const src = f.preview_url || f.media_url
  authBlob(src).then(function (url) {
    if (kind === 'img') {
      node.outerHTML = '<img class="lb-img" src="' + url + '" alt="" />'
    } else if (kind === 'video') {
      node.outerHTML = '<video class="lb-video" controls autoplay src="' + url + '"></video>'
    } else if (kind === 'audio') {
      node.outerHTML = '<div class="lb-file"><span class="emoji">🎵</span><p class="muted">' + esc(f.name) +
        '</p><audio controls src="' + url + '"></audio></div>'
    }
  }).catch(function () {
    node.outerHTML =
      '<div class="lb-file"><span class="emoji">⚠️</span><p class="muted">' + esc(f.name) + '</p>' +
      '<p class="muted small">预览加载失败</p><button class="btn btn-primary btn-sm" id="lb-fallback-dl">⬇ 下载查看</button></div>'
  })
}

function renderPreview(f) {
  const ext = extOf(f.name)
  if (isImage(f.name)) {
    return '<div class="lb-loading" data-preview-src="img"><div class="spinner"></div><p class="muted small">加载预览…</p></div>'
  }
  if (ext === 'mp4' || ext === 'mov' || ext === 'webm' || ext === 'm4v') {
    return '<div class="lb-loading" data-preview-src="video"><div class="spinner"></div><p class="muted small">加载视频…</p></div>'
  }
  if (ext === 'mp3' || ext === 'wav' || ext === 'flac' || ext === 'ogg') {
    return '<div class="lb-loading" data-preview-src="audio"><div class="spinner"></div></div>'
  }
  if (ext === 'pdf') {
    return '<div class="lb-file"><span class="emoji">📕</span><p class="muted">' + esc(f.name) +
      '</p><button class="btn btn-primary btn-sm" id="lb-open-pdf">↗ 新窗口打开 / 下载</button></div>'
  }
  return '<div class="lb-file"><span class="emoji">' + fileIcon(f.name) + '</span><p class="muted">' +
    esc(f.name) + '（' + formatSize(f.size) + '）</p><p class="muted small">该类型暂不支持在线预览，请直接下载</p></div>'
}

function downloadFile(f) {
  const a = document.createElement('a')
  a.href = authUrl(f.download_url)
  a.download = f.name
  document.body.appendChild(a)
  a.click()
  a.remove()
}

/* ----------------------------- wire-up ----------------------------- */
els.loginForm.addEventListener('submit', async (e) => {
  e.preventDefault()
  els.loginErr.classList.add('hidden')
  const btn = document.getElementById('login-btn')
  btn.disabled = true
  try {
    await login(els.username.value.trim(), els.password.value)
    toast('登录成功')
    showApp()
  } catch (err) {
    els.loginErr.textContent = err.message
    els.loginErr.classList.remove('hidden')
  } finally {
    btn.disabled = false
  }
})
$('logout-btn').addEventListener('click', () => {
  showLogin()
  els.password.value = ''
  toast('已退出登录')
})
els.lbDownload.addEventListener('click', () => {
  const f = state.files[state.lbIndex]
  if (f) downloadFile(f)
})
const lbCloseBtn = document.getElementById('lb-close')
lbCloseBtn.addEventListener('click', closeLightbox)
els.lbPrev.addEventListener('click', () => { state.lbIndex--; refreshLightbox() })
els.lbNext.addEventListener('click', () => { state.lbIndex++; refreshLightbox() })
document.addEventListener('keydown', (e) => {
  if (els.lightbox.classList.contains('hidden')) return
  if (e.key === 'Escape') closeLightbox()
  if (e.key === 'ArrowLeft') { state.lbIndex--; refreshLightbox() }
  if (e.key === 'ArrowRight') { state.lbIndex++; refreshLightbox() }
})
els.retryBtn.addEventListener('click', loadCurrent)
document.getElementById('sel-all').addEventListener('change', function (e) {
  if (e.target.checked) state.files.forEach(function (f) { state.selected.add(f.id) })
  else state.selected.clear()
  renderGrids()
})
document.getElementById('batch-dl').addEventListener('click', batchDownload)
els.lightbox.addEventListener('click', (e) => {
  if (e.target === els.lightbox) closeLightbox()
})

/* PDF / 预览失败回退按钮（delegated） */
els.lbBody.addEventListener('click', (e) => {
  const f = state.files[state.lbIndex]
  if (!f) return
  const pdf = e.target.closest('#lb-open-pdf')
  const dl = e.target.closest('#lb-fallback-dl')
  if (pdf) { window.open(authUrl(f.media_url || f.download_url), '_blank'); return }
  if (dl) downloadFile(f)
})

function esc(s) {
  return String(s == null ? '' : s)
    .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;').replace(/'/g, '&#39;')
}

/* ----------------------------- boot ----------------------------- */
;(async function init() {
  const token = localStorage.getItem(TOKEN_KEY)
  if (!token) { showLogin(); return }
  try {
    state.user = await fetchMe()
    showApp()
  } catch {
    showLogin()
  }
})()