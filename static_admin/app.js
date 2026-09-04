'use strict'

/* ===========================================================================
   管理端前端 (端口 8002) — 超级管理员登录 / 新建与管理普通用户 / 批量上传
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
  stats: $('stats'),
  userRows: $('user-rows'),
  emptyUsers: $('empty-users'),
  loading: $('loading'),
  error: $('error'),
  errorMsg: $('error-msg'),
  toast: $('toast'),
}

let state = {
  user: null,
  users: [],
  toastTimer: null,
  upFiles: [],
}

/* ----------------------------- helpers ----------------------------- */
async function api(path, options = {}) {
  const headers = options.headers || {}
  const token = localStorage.getItem(TOKEN_KEY)
  if (token) headers.Authorization = 'Bearer ' + token
  if (options.body && typeof options.body !== 'string' && !(options.body instanceof FormData)) {
    headers['Content-Type'] = 'application/json'
    options.body = JSON.stringify(options.body)
  }
  const res = await fetch(path, { ...options, headers })
  let data = null
  try { data = await res.json() } catch { /* ignore */ }
  if (!res.ok || (data && data.success === false)) {
    const msg = (data && (data.error || data.message)) || '请求失败 (' + res.status + ')'
    if (res.status === 401) showLogin()
    const e = new Error(msg)
    e.status = res.status
    throw e
  }
  return data ? data.data : null
}

function toast(msg) {
  els.toast.textContent = msg
  els.toast.classList.remove('hidden')
  clearTimeout(state.toastTimer)
  state.toastTimer = setTimeout(() => els.toast.classList.add('hidden'), 2800)
}

function esc(s) {
  return String(s == null ? '' : s)
    .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;').replace(/'/g, '&#39;')
}

/* ---------------------- expiry helpers ---------------------- */
function nowMs() { return Date.now() }

function expiryInfo(expiresAt) {
  if (!expiresAt) {
    return { text: '永久有效', cls: 'badge-muted' }
  }
  const t = new Date(expiresAt.replace('T', ' ').replace(' ', 'T'))
  const now = nowMs()
  if (Number.isNaN(t.getTime())) {
    return { text: esc(expiresAt), cls: 'badge-muted' }
  }
  if (t.getTime() <= now) {
    return { text: '已过期', cls: 'badge-danger' }
  }
  const days = Math.ceil((t.getTime() - now) / 86400000)
  if (days <= 7) return { text: days + ' 天', cls: 'badge-warn' }
  return { text: days + ' 天', cls: 'badge-ok' }
}

function toLocalInput(value) {
  if (!value) return ''
  const s = String(value).replace('T', ' ')
  const m = s.match(/^(\d{4}-\d{2}-\d{2}) (\d{2}:\d{2})/)
  return m ? m[1] + 'T' + m[2] : ''
}

function formatDate(input) {
  if (!input) return ''
  const d = new Date(String(input).replace('T', ' ').replace(' ', 'T'))
  if (Number.isNaN(d.getTime())) return esc(input)
  const p = (x) => String(x).padStart(2, '0')
  return d.getFullYear() + '-' + p(d.getMonth() + 1) + '-' + p(d.getDate()) +
    ' ' + p(d.getHours()) + ':' + p(d.getMinutes())
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
  els.userName.textContent = state.user ? state.user.username : ''
  loadAll()
}

/* ----------------------------- load ----------------------------- */
async function loadAll() {
  els.loading.classList.remove('hidden')
  els.error.classList.add('hidden')
  try {
    await Promise.all([loadUsers(), loadStats()])
  } catch (err) {
    els.errorMsg.textContent = err.message
    els.error.classList.remove('hidden')
  } finally {
    els.loading.classList.add('hidden')
  }
}

async function loadUsers() {
  state.users = (await api('/api/admin/users')) || []
  renderUsers()
}
async function loadStats() {
  const s = (await api('/api/admin/stats')) || {}
  const cards = [
    { icon: '👥', label: '用户', v: s.users ?? 0 },
    { icon: '📄', label: '文件', v: s.files ?? 0 },
    { icon: '📁', label: '文件夹', v: s.folders ?? 0 },
    { icon: '💾', label: '总存储', v: s.formatted_size || '0 B' },
  ]
  els.stats.innerHTML = cards.map((c) =>
    '<div class="stat-card card"><span class="icon">' + c.icon + '</span>' +
    '<div><strong>' + esc(c.v) + '</strong><small>' + esc(c.label) + '</small></div></div>'
  ).join('')
  els.stats.classList.remove('hidden')
}

function renderUsers() {
  els.userRows.innerHTML = ''
  els.emptyUsers.classList.toggle('hidden', state.users.length !== 0)
  for (const u of state.users) {
    els.userRows.appendChild(userRow(u))
  }
}

function userRow(u) {
  const exp = expiryInfo(u.expires_at)
  const tr = document.createElement('tr')

  const expCell = document.createElement('td')
  expCell.innerHTML = '<span class="badge ' + exp.cls + '">' + exp.text + '</span>' +
    (u.expires_at ? '<div class="muted small">' + formatDate(u.expires_at) + '</div>' : '')

  const op = document.createElement('td')
  op.className = 'op'
  op.innerHTML =
    '<button class="btn btn-sm btn-ghost" data-act="upload">⬆ 上传</button>' +
    '<button class="btn btn-sm btn-ghost" data-act="edit">编辑</button>' +
    '<button class="btn btn-sm btn-danger" data-act="del">删除</button>'
  op.addEventListener('click', (e) => {
    const act = e.target.closest('button') && e.target.closest('button').dataset.act
    if (act === 'upload') openUpload(u)
    else if (act === 'edit') openEdit(u)
    else if (act === 'del') delUser(u)
  })

  tr.innerHTML =
    '<td class="muted">' + u.id + '</td>' +
    '<td class="uname">' + esc(u.username) + '</td>' +
    '<td><span class="badge">' + (u.role === 'admin' ? '管理员' : '普通用户') + '</span></td>'
  tr.appendChild(expCell)
  tr.innerHTML +=
    '<td>' + (u.file_count || 0) + '</td>' +
    '<td class="muted small">' + formatDate(u.created_at) + '</td>'
  tr.appendChild(op)
  return tr
}

/* ----------------------------- create ----------------------------- */
function openCreate() {
  $('cu-username').value = ''
  $('cu-password').value = ''
  $('cu-expires').value = ''
  $('cu-role').value = 'user'
  $('create-err').classList.add('hidden')
  $('create-modal').classList.remove('hidden')
  $('cu-username').focus()
}
async function submitCreate() {
  const username = $('cu-username').value.trim()
  const password = $('cu-password').value
  const errEl = $('create-err')
  if (!username) return fail(errEl, '请输入用户名')
  if (password.length < 6) return fail(errEl, '密码至少 6 位')
  const expires = $('cu-expires').value || null
  try {
    await api('/api/admin/users', {
      method: 'POST',
      body: {
        username,
        password,
        role: $('cu-role').value,
        expires_at: expires, // null = 永久有效
      },
    })
    $('create-modal').classList.add('hidden')
    toast('用户「' + username + '」已创建，并自动建立「原图」文件夹')
    await loadUsers()
  } catch (err) {
    fail(errEl, err.message)
  }
}
function fail(el, msg) {
  el.textContent = msg
  el.classList.remove('hidden')
}

/* ----------------------------- edit ----------------------------- */
let editingUser = null
function openEdit(u) {
  editingUser = u
  $('eu-username').value = u.username
  $('eu-password').value = ''
  $('eu-role').value = u.role
  $('eu-expires').value = toLocalInput(u.expires_at)
  $('eu-keep').checked = true
  $('edit-err').classList.add('hidden')
  $('edit-modal').classList.remove('hidden')
}
async function submitEdit() {
  const errEl = $('edit-err')
  const keep = $('eu-keep').checked
  const body = {}
  const username = $('eu-username').value.trim()
  if (!username) return fail(errEl, '用户名不能为空')
  body.username = username
  const password = $('eu-password').value
  if (password) {
    if (password.length < 6) return fail(errEl, '密码至少 6 位')
    body.password = password
  }
  body.role = $('eu-role').value
  if (!keep) {
    // 不保留原有效期：以输入框值为准；为空则清除（永久有效）
    body.expires_at = $('eu-expires').value || null
  }
  try {
    await api('/api/admin/users/' + editingUser.id, { method: 'PUT', body })
    $('edit-modal').classList.add('hidden')
    toast('用户已更新')
    await loadUsers()
  } catch (err) {
    fail(errEl, err.message)
  }
}

/* ----------------------------- delete ----------------------------- */
async function delUser(u) {
  const ok = window.confirm(
    '确定删除用户「' + u.username + '」？\n其全部文件与文件夹将被一并删除，无法恢复。'
  )
  if (!ok) return
  try {
    await api('/api/admin/users/' + u.id, { method: 'DELETE' })
    toast('用户已删除')
    await loadAll()
  } catch (err) {
    toast(err.message)
  }
}

/* ----------------------------- upload ----------------------------- */
let upTarget = null
let upFolders = []
function openUpload(u) {
  upTarget = u
  $('up-username').textContent = u.username
  const bar = $('up-progress')
  if (!bar.firstElementChild) bar.innerHTML = '<div></div>'
  bar.classList.remove('hidden')
  bar.firstElementChild.style.width = '0%'
  $('up-list').innerHTML = ''
  $('up-result').classList.add('hidden')
  $('up-input').value = ''
  state.upFiles = []
  $('upload-modal').classList.remove('hidden')
  loadUploadFolders()
}

async function collectFolderTree(userId) {
  const list = []
  async function walk(parentId, depth) {
    if (depth > 8) return
    const res = await api('/api/admin/users/' + userId + '/folders' + (parentId ? '?parent_id=' + parentId : ''))
    const folders = (res && res.folders) || []
    for (const f of folders) {
      list.push({ id: f.id, name: f.name, depth, parentId })
      await walk(f.id, depth + 1)
    }
  }
  await walk(null, 0)
  return list
}

async function loadUploadFolders() {
  if (!upTarget) return
  const sel = $('up-folder-select')
  try {
    upFolders = await collectFolderTree(upTarget.id)
  } catch (err) {
    toast(err.message)
    upFolders = []
  }
  sel.innerHTML = ''
  const root = document.createElement('option')
  root.value = ''
  root.textContent = '🏠 根目录'
  sel.appendChild(root)
  for (const f of upFolders) {
    const opt = document.createElement('option')
    opt.value = String(f.id)
    opt.textContent = (f.depth ? '　'.repeat(f.depth) + '└ ' : '') + f.name
    sel.appendChild(opt)
  }
  // 默认选中「原图」文件夹（若存在）
  if (upTarget.original_folder_id) {
    const has = upFolders.some((f) => f.id === upTarget.original_folder_id)
    if (has) sel.value = String(upTarget.original_folder_id)
  }
  updateUploadFolderNote()
}

async function createFolderForTarget() {
  if (!upTarget) return
  const sel = $('up-folder-select')
  const parentId = sel.value ? Number(sel.value) : null
  const name = window.prompt('新建文件夹名称：')
  if (name == null) return
  const trimmed = name.trim()
  if (!trimmed) { toast('请输入文件夹名称'); return }
  try {
    const created = await api('/api/admin/users/' + upTarget.id + '/folders', {
      method: 'POST',
      body: { name: trimmed, parent_id: parentId },
    })
    toast('文件夹「' + trimmed + '」已创建')
    await loadUploadFolders()
    if (created && created.id) sel.value = String(created.id)
  } catch (err) {
    toast(err.message)
  }
}

function updateUploadFolderNote() {
  $('up-folder-note').textContent = $('up-folder-select').value
    ? '将上传到该用户所选文件夹'
    : '将上传到该用户的根目录'
}

function addFiles(fileList) {
  const arr = Array.from(fileList || [])
  if (!arr.length) return
  state.upFiles = state.upFiles.concat(arr)
  $('up-list').innerHTML = ''
  for (const f of state.upFiles) {
    const li = document.createElement('li')
    li.innerHTML = '<span class="name">' + esc(f.name) + '</span><span class="state-tag muted">待上传</span>'
    li.dataset.name = f.name
    $('up-list').appendChild(li)
  }
  if (state.upFiles.length) uploadAll()
}

function uploadAll() {
  const files = state.upFiles
  if (!files.length) return
  const form = new FormData()
  for (const f of files) form.append('file', f, f.name)
  const selVal = $('up-folder-select').value
  if (selVal) form.append('folder_id', selVal)
  form.append('user_id', String(upTarget.id))

  const bar = $('up-progress')
  if (!bar.firstElementChild) bar.innerHTML = '<div></div>'
  const fill = bar.firstElementChild
  bar.classList.remove('hidden')

  const xhr = new XMLHttpRequest()
  const token = localStorage.getItem(TOKEN_KEY)
  xhr.open('POST', '/api/files/upload')
  if (token) xhr.setRequestHeader('Authorization', 'Bearer ' + token)
  xhr.upload.onprogress = (e) => {
    if (e.lengthComputable) fill.style.width = Math.round((e.loaded / e.total) * 100) + '%'
  }
  xhr.onload = () => {
    let body = null
    try { body = JSON.parse(xhr.responseText) } catch { /* ignore */ }
    if (xhr.status >= 200 && xhr.status < 300 && body && body.success) {
      const uploaded = (body.data && body.data.files) || []
      const errs = (body.data && body.data.errors) || []
      const uploadedNames = new Set(uploaded.map((f) => f.name))
      // 逐项标注结果
      $('up-list').querySelectorAll('li').forEach((li) => {
        const tag = li.querySelector('.state-tag')
        if (uploadedNames.has(li.dataset.name)) {
          tag.textContent = '✓ 已上传'
          tag.className = 'state-tag ok'
        } else {
          tag.textContent = '⚠ 已跳过'
          tag.className = 'state-tag fail'
        }
      })
      fill.style.width = '100%'
      const result = $('up-result')
      result.textContent = '上传完成：成功 ' + uploaded.length + ' 个' +
        (errs.length ? '，跳过 ' + errs.length + ' 个（已存在或类型受限）' : '')
      result.classList.remove('hidden')
      errs.slice(0, 2).forEach((t) => toast(t))
      state.upFiles = []
      loadUsers()
    } else {
      toast((body && body.error) || '上传失败 (' + xhr.status + ')')
    }
  }
  xhr.onerror = () => toast('网络错误，上传失败')
  xhr.send(form)
}

/* ----------------------------- wire-up ----------------------------- */
els.loginForm.addEventListener('submit', async (e) => {
  e.preventDefault()
  els.loginErr.classList.add('hidden')
  const btn = document.getElementById('login-btn')
  btn.disabled = true
  try {
    const data = await api('/api/auth/login', {
      method: 'POST',
      body: { username: els.username.value.trim(), password: els.password.value },
    })
    localStorage.setItem(TOKEN_KEY, data.token)
    state.user = data.user
    // 校验管理员身份
    const me = await api('/api/auth/me')
    if (!me || me.role !== 'admin') {
      localStorage.removeItem(TOKEN_KEY)
      throw new Error('该账号不是管理员，请使用具备管理员权限的账户登录')
    }
    state.user = me
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
$('retry-btn').addEventListener('click', loadAll)
$('refresh-btn').addEventListener('click', loadAll)
$('create-btn').addEventListener('click', openCreate)
$('create-save').addEventListener('click', submitCreate)
$('cu-clear-exp').addEventListener('click', () => { $('cu-expires').value = '' })
$('edit-save').addEventListener('click', submitEdit)
$('eu-permanent').addEventListener('click', () => {
  $('eu-expires').value = ''
  $('eu-keep').checked = false
})

$('up-input').addEventListener('change', (e) => addFiles(e.target.files))
$('up-drop').addEventListener('click', () => $('up-input').click())
$('up-newfolder').addEventListener('click', createFolderForTarget)
$('up-folder-select').addEventListener('change', updateUploadFolderNote)
const dropEl = $('up-drop')
dropEl.addEventListener('dragover', (e) => { e.preventDefault(); dropEl.classList.add('drag') })
dropEl.addEventListener('dragleave', () => dropEl.classList.remove('drag'))
dropEl.addEventListener('drop', (e) => {
  e.preventDefault()
  dropEl.classList.remove('drag')
  addFiles(e.dataTransfer.files)
})

/* close any modal via [data-close] or backdrop */
document.querySelectorAll('.modal-wrap').forEach((wrap) => {
  wrap.addEventListener('click', (e) => {
    if (e.target === wrap || e.target.closest('[data-close]')) wrap.classList.add('hidden')
  })
})
document.querySelectorAll('#create-err,#edit-err').forEach((el) => el.classList.add('hidden'))

/* ----------------------------- boot ----------------------------- */
;(async function init() {
  const token = localStorage.getItem(TOKEN_KEY)
  if (!token) { showLogin(); return }
  try {
    const me = await api('/api/auth/me')
    if (!me || me.role !== 'admin') throw new Error('not admin')
    state.user = me
    showApp()
  } catch {
    showLogin()
  }
})()