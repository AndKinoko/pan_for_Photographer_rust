// ===== API Client =====
const API_BASE = '/api';
let authToken = localStorage.getItem('auth_token');
let currentUser = null;

// ===== Batch Selection State =====
let selectedItems = new Map(); // key: "file_N" | "folder_N", value: {id, name, type}

// ===== Shared Format Constants =====
const IMAGE_FORMATS = ['jpg','jpeg','png','gif','bmp','webp','tiff','tif'];
const RAW_FORMATS = ['nef','cr2','cr3','crw','arw','sr2','srf','dng','raf','orf','rw2','nrw'];
const VIDEO_FORMATS = ['mp4','avi','mov','wmv','flv','mkv','webm'];
const ARCHIVE_FORMATS = ['zip','rar','7z','tar','gz'];
const DOC_FORMATS = ['doc','docx'];
const SPREADSHEET_FORMATS = ['xls','xlsx'];

function isImageFormat(ft) { return IMAGE_FORMATS.includes(ft) || RAW_FORMATS.includes(ft); }
function isVideoFormat(ft) { return VIDEO_FORMATS.includes(ft); }

async function api(path, options = {}) {
    const headers = { ...options.headers };
    if (authToken) headers['Authorization'] = `Bearer ${authToken}`;
    if (!(options.body instanceof FormData)) {
        headers['Content-Type'] = 'application/json';
    }

    let res;
    try {
        res = await fetch(API_BASE + path, { ...options, headers });
    } catch (err) {
        throw new Error('无法连接服务器，请检查服务是否启动');
    }

    if (res.status === 401) {
        authToken = null;
        localStorage.removeItem('auth_token');
        currentUser = null;
        showPage('auth');
        throw new Error('认证失败');
    }

    const contentType = res.headers.get('content-type') || '';
    if (!contentType.includes('application/json')) {
        const text = await res.text();
        throw new Error(`服务器返回异常 (${res.status}): ${text.substring(0, 100)}`);
    }

    let data;
    try {
        data = await res.json();
    } catch (err) {
        throw new Error('服务器返回数据格式异常');
    }

    if (!data.success && data.error) {
        throw new Error(data.error);
    }
    return data;
}

// ===== Toast =====
function showToast(message, type = 'info') {
    const container = document.getElementById('toast-container');
    const toast = document.createElement('div');
    toast.className = `toast ${type}`;
    toast.textContent = message;
    container.appendChild(toast);
    setTimeout(() => { toast.remove(); }, 3500);
}

// ===== User Dropdown =====
function toggleUserDropdown() {
    const menu = document.getElementById('user-dropdown-menu');
    const btn = document.getElementById('user-dropdown-btn');
    const isOpen = menu.classList.contains('show');
    menu.classList.toggle('show', !isOpen);
    btn.classList.toggle('active', !isOpen);
}

// Close dropdown when clicking outside
document.addEventListener('click', function(e) {
    const dropdown = document.getElementById('user-dropdown-menu');
    const btn = document.getElementById('user-dropdown-btn');
    if (dropdown && btn && !btn.contains(e.target) && !dropdown.contains(e.target)) {
        dropdown.classList.remove('show');
        btn.classList.remove('active');
    }
});

// ===== Page Navigation =====
function showPage(name, data) {
    document.querySelectorAll('.page').forEach(p => p.style.display = 'none');
    const page = document.getElementById(`page-${name}`);
    if (page) page.style.display = '';

    // Update nav links
    document.querySelectorAll('.nav-link').forEach(l => {
        l.classList.toggle('active', l.dataset.page === name);
    });

    // Show/hide nav
    const nav = document.getElementById('nav-links');
    const navUser = document.getElementById('nav-user');
    if (name === 'auth') {
        nav.style.display = 'none';
        navUser.style.display = 'none';
    } else {
        nav.style.display = '';
        navUser.style.display = '';
    }

    // Load page data
    switch (name) {
        case 'home': loadHome(data?.folder_id); break;
        case 'upload': initUploadPage(); break;
        case 'search': loadSearchPage(); break;
        case 'shares': loadShares(); break;
        case 'share-public': loadPublicShare(data?.share_id); break;
    }
}

// ===== Auth =====
document.addEventListener('DOMContentLoaded', () => {
    // Auth tab switching
    document.querySelectorAll('.auth-tab').forEach(tab => {
        tab.addEventListener('click', () => {
            switchAuthTab(tab.dataset.tab);
        });
    });

    // Login form
    document.getElementById('login-form').addEventListener('submit', async (e) => {
        e.preventDefault();
        const form = e.target;
        const username = form.username.value.trim();
        const password = form.password.value;
        const errorEl = document.getElementById('auth-error');
        errorEl.textContent = '';

        if (!username) { errorEl.textContent = '请输入用户名'; return; }
        if (!password) { errorEl.textContent = '请输入密码'; return; }

        try {
            const data = await api('/auth/login', {
                method: 'POST',
                body: JSON.stringify({ username, password }),
            });
            authToken = data.data.token;
            currentUser = data.data.user;
            localStorage.setItem('auth_token', authToken);
            updateUserDisplay();
            showToast('登录成功', 'success');
            showPage('home');
        } catch (err) {
            errorEl.textContent = err.message;
        }
    });

    // Register form
    document.getElementById('register-form').addEventListener('submit', async (e) => {
        e.preventDefault();
        const form = e.target;
        const username = form.username.value.trim();
        const password = form.password.value;
        const confirm = form.confirm_password.value;
        const errorEl = document.getElementById('auth-error');
        errorEl.textContent = '';

        if (!username) { errorEl.textContent = '请输入用户名'; return; }
        if (!password) { errorEl.textContent = '请输入密码'; return; }
        if (password.length < 6) { errorEl.textContent = '密码至少6位'; return; }
        if (password !== confirm) { errorEl.textContent = '两次输入的密码不一致'; return; }

        try {
            const data = await api('/auth/register', {
                method: 'POST',
                body: JSON.stringify({ username, password }),
            });
            authToken = data.data.token;
            currentUser = data.data.user;
            localStorage.setItem('auth_token', authToken);
            updateUserDisplay();
            showToast('注册成功', 'success');
            showPage('home');
        } catch (err) {
            errorEl.textContent = err.message;
        }
    });

    // Skip auth check for share/public routes
    const isSharePage = window.location.pathname.startsWith('/share/');
    if (!isSharePage) {
        // Check existing auth
        if (authToken) {
            checkAuth();
        } else {
            showPage('auth');
        }
    }

    // Search on Enter key
    const searchInput = document.getElementById('search-input');
    if (searchInput) {
        searchInput.addEventListener('keydown', (e) => {
            if (e.key === 'Enter') doSearch();
        });
    }
});

function updateUserDisplay() {
    if (currentUser) {
        document.getElementById('user-name-display').textContent = currentUser.username;
        const avatar = document.getElementById('user-avatar-letter');
        if (avatar && currentUser.username) {
            avatar.textContent = currentUser.username.charAt(0).toUpperCase();
        }
    }
}

function switchAuthTab(tabName) {
    const isLogin = tabName === 'login';

    // Update tabs
    document.querySelectorAll('.auth-tab').forEach(t => t.classList.remove('active'));
    document.querySelector(`.auth-tab[data-tab="${tabName}"]`).classList.add('active');

    // Update forms
    document.querySelectorAll('.auth-form').forEach(f => f.classList.remove('active'));
    document.getElementById(`${tabName}-form`).classList.add('active');

    // Clear errors
    document.getElementById('auth-error').textContent = '';

    // Update auth title
    document.getElementById('auth-title-text').textContent = isLogin ? '欢迎回来' : '创建账号';
    document.getElementById('auth-subtitle-text').textContent = isLogin ? '登录您的账号以继续使用' : '注册一个新账号以使用网盘服务';

    // Show/hide divider and switch buttons
    const dividerLogin = document.getElementById('auth-divider-login');
    const dividerRegister = document.getElementById('auth-divider-register');
    const switchToRegister = document.getElementById('auth-switch-to-register');
    const switchToLogin = document.getElementById('auth-switch-to-login');
    const forgotPassword = document.getElementById('auth-forgot-password');

    if (dividerLogin) dividerLogin.style.display = isLogin ? '' : 'none';
    if (dividerRegister) dividerRegister.style.display = isLogin ? 'none' : '';
    if (switchToRegister) switchToRegister.style.display = isLogin ? '' : 'none';
    if (switchToLogin) switchToLogin.style.display = isLogin ? 'none' : '';
    if (forgotPassword) forgotPassword.style.display = isLogin ? '' : 'none';

    // Show/hide privacy card
    const privacyCard = document.getElementById('privacy-card');
    if (privacyCard) {
        privacyCard.style.display = isLogin ? 'none' : '';
    }
}

async function checkAuth() {
    try {
        const data = await api('/auth/me');
        currentUser = data.data;
        updateUserDisplay();
        showPage('home');
    } catch {
        authToken = null;
        localStorage.removeItem('auth_token');
        showPage('auth');
    }
}

function logout() {
    authToken = null;
    currentUser = null;
    localStorage.removeItem('auth_token');
    document.getElementById('user-dropdown-menu').classList.remove('show');
    document.getElementById('user-dropdown-btn').classList.remove('active');
    showPage('auth');
    showToast('已退出登录', 'info');
}

// ===== Password Toggle =====
function togglePassword(btn) {
    const input = btn.parentElement.querySelector('input');
    const eyeOff = btn.querySelector('.eye-off');
    const eyeOn = btn.querySelector('.eye-on');
    if (input.type === 'password') {
        input.type = 'text';
        if (eyeOff) eyeOff.style.display = 'none';
        if (eyeOn) eyeOn.style.display = '';
    } else {
        input.type = 'password';
        if (eyeOff) eyeOff.style.display = '';
        if (eyeOn) eyeOn.style.display = 'none';
    }
}

// ===== Password Strength Meter =====
function updatePasswordStrength() {
    const pw = document.getElementById('register-password');
    const fill = document.getElementById('strength-fill');
    const text = document.getElementById('strength-text');
    if (!pw || !fill || !text) return;

    const val = pw.value;
    let score = 0;
    if (val.length >= 6) score++;
    if (val.length >= 10) score++;
    if (/[a-z]/.test(val) && /[A-Z]/.test(val)) score++;
    if (/\d/.test(val)) score++;
    if (/[^a-zA-Z0-9]/.test(val)) score++;

    const levels = [
        { width: '20%', color: '#ef4444', label: '很弱' },
        { width: '40%', color: '#f97316', label: '弱' },
        { width: '60%', color: '#eab308', label: '一般' },
        { width: '80%', color: '#84cc16', label: '强' },
        { width: '100%', color: '#10b981', label: '很强' },
    ];
    const idx = Math.min(score, levels.length - 1);
    fill.style.width = levels[idx].width;
    fill.style.background = levels[idx].color;
    text.textContent = levels[idx].label;
    text.style.color = levels[idx].color;

    if (val.length === 0) {
        fill.style.width = '0';
        text.textContent = '';
    }
}

function validateConfirmPassword() {
    const pw = document.getElementById('register-password');
    const confirm = document.getElementById('register-confirm');
    const match = document.getElementById('confirm-match');
    const hint = document.getElementById('confirm-hint');
    if (!pw || !confirm || !match || !hint) return;

    if (confirm.value.length > 0 && pw.value === confirm.value) {
        match.style.display = '';
        hint.style.color = '#10b981';
        hint.textContent = '密码匹配';
    } else if (confirm.value.length > 0) {
        match.style.display = 'none';
        hint.style.color = '#ef4444';
        hint.textContent = '密码不匹配';
    } else {
        match.style.display = 'none';
        hint.style.color = '';
        hint.textContent = '再次输入密码以确认';
    }
}

// ===== Home =====
let currentFolderId = null;
let previewFileId = null;
let previewFileName = null;

async function loadHome(folderId = null) {
    currentFolderId = folderId;
    clearSelection();
    const folderList = document.getElementById('folder-list');
    const fileList = document.getElementById('file-list');
    const emptyState = document.getElementById('empty-state');
    const breadcrumb = document.getElementById('breadcrumb');

    try {
        const parentParam = folderId != null ? `?parent_id=${folderId}` : '';
        const folderParam = folderId != null ? `?folder_id=${folderId}` : '';
        const [foldersData, filesData] = await Promise.all([
            api(`/folders${parentParam}`),
            api(`/files${folderParam}`),
        ]);

        const folders = foldersData.data.folders || [];
        const files = filesData.data || [];
        const breadcrumbs = foldersData.data.breadcrumbs || [];

        // Breadcrumb
        breadcrumb.innerHTML = `<a href="#" onclick="loadHome()">根目录</a>` +
            breadcrumbs.map(f => ` &rsaquo; <a href="#" onclick="loadHome(${f.id})">${escHtml(f.name)}</a>`).join('');

        // Folders
        if (folders.length > 0) {
            folderList.innerHTML = folders.map(f => `
                <div class="folder-card">
                    <div class="folder-icon">
                        <svg viewBox="0 0 24 24" fill="none" stroke="#f59e0b" stroke-width="1.5">
                            <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>
                        </svg>
                    </div>
                    <div class="folder-name" title="${escHtml(f.name)}">${escHtml(f.name)}</div>
                    <div class="folder-actions">
                        <button class="btn btn-sm btn-outline" onclick="event.stopPropagation();loadHome(${f.id})">打开</button>
                        <button class="btn btn-sm btn-danger" onclick="event.stopPropagation();deleteFolder(${f.id},'${escHtml(f.name)}')">删除</button>
                    </div>
                </div>
            `).join('');
            folderList.style.display = '';
        } else {
            folderList.style.display = 'none';
        }

        // Files
        if (files.length > 0) {
            fileList.innerHTML = `
                <table class="file-table">
                    <thead>
                        <tr>
                            <th class="file-checkbox-cell"></th>
                            <th>文件名</th>
                            <th>类型</th>
                            <th>大小</th>
                            <th>上传时间</th>
                            <th>操作</th>
                        </tr>
                    </thead>
                    <tbody>
                        ${files.map(f => {
                            const thumbHtml = f.thumb_url
                                ? `<img class="file-thumb" src="${f.thumb_url}&token=${encodeURIComponent(authToken || '')}" alt="" loading="lazy" onerror="this.style.display='none';this.parentElement.classList.add('${getFileIconClass(f.file_type)}');this.parentElement.innerHTML='${getFileIconSvg(f.file_type)}';">`
                                : getFileIconSvg(f.file_type);
                            const iconClass = f.thumb_url ? '' : getFileIconClass(f.file_type);
                            const itemKey = `file_${f.id}`;
                            const isChecked = selectedItems.has(itemKey);
                            return `
                            <tr class="file-row" data-file-id="${f.id}"
                                data-file-name="${escHtml(f.name)}"
                                data-file-type="${f.file_type}"
                                data-file-url="${API_BASE}/files/${f.id}/download"
                                data-preview-url="${f.preview_url || ''}"
                                data-thumb-url="${f.thumb_url || ''}"
                                data-file-size="${f.formatted_size}"
                                data-file-uploaded="${f.uploaded_at}">
                                <td class="file-checkbox-cell" onclick="event.stopPropagation()">
                                    <input type="checkbox" class="file-checkbox" data-key="${itemKey}" data-id="${f.id}" data-type="file" data-name="${escHtml(f.name)}" ${isChecked ? 'checked' : ''} onchange="onItemCheckbox(this)">
                                </td>
                                <td>
                                    <div class="file-name-cell">
                                        <div class="file-icon ${iconClass}">
                                            ${thumbHtml}
                                        </div>
                                        <div>
                                            <div class="fw-bold" style="font-size:0.9rem">${escHtml(f.name)}</div>
                                            <small style="color:var(--text-muted);font-size:0.75rem">原名: ${escHtml(f.original_name)}</small>
                                        </div>
                                    </div>
                                </td>
                                <td><span class="file-type-badge">${f.file_type.toUpperCase()}</span></td>
                                <td style="color:var(--text-secondary);font-size:0.85rem">${f.formatted_size}</td>
                                <td style="color:var(--text-muted);font-size:0.85rem">${f.uploaded_at}</td>
                                <td style="text-align:right">
                                    <div class="file-actions" style="justify-content:flex-end">
                                        <a class="btn btn-sm btn-outline" href="${API_BASE}/files/${f.id}/download?token=${encodeURIComponent(authToken || '')}" title="下载">
                                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width:14px;height:14px;"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/></svg>
                                        </a>
                                        <button class="btn btn-sm btn-outline preview-btn" title="预览"
                                            data-file-id="${f.id}"
                                            data-file-name="${escHtml(f.name)}"
                                            data-file-type="${f.file_type}"
                                            data-file-url="${API_BASE}/files/${f.id}/download"
                                            data-preview-url="${f.preview_url || ''}"
                                            data-thumb-url="${f.thumb_url || ''}"
                                            data-file-size="${f.formatted_size}"
                                            data-file-uploaded="${f.uploaded_at}">
                                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width:14px;height:14px;"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/></svg>
                                        </button>
                                        <button class="btn btn-sm btn-outline" onclick="showShareModal(${f.id},'${escHtml(f.name)}')" title="分享">
                                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width:14px;height:14px;"><circle cx="18" cy="5" r="3"/><circle cx="6" cy="12" r="3"/><circle cx="18" cy="19" r="3"/><line x1="8.59" y1="13.51" x2="15.42" y2="17.49"/><line x1="15.41" y1="6.51" x2="8.59" y2="10.49"/></svg>
                                        </button>
                                        <button class="btn btn-sm btn-danger" onclick="deleteFile(${f.id},'${escHtml(f.name)}')" title="删除">
                                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width:14px;height:14px;"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
                                        </button>
                                    </div>
                                </td>
                            </tr>
                        `}).join('')}
                    </tbody>
                </table>
            `;
            fileList.style.display = '';
            emptyState.style.display = 'none';

            // Attach file row click handlers
            attachFileRowHandlers();
        } else {
            fileList.style.display = 'none';
            emptyState.style.display = '';
        }

        // Reset preview panel
        resetPreviewPanel();
    } catch (err) {
        showToast(err.message, 'error');
    }
}

function attachFileRowHandlers() {
    const fileRows = document.querySelectorAll('.file-row');
    const previewButtons = document.querySelectorAll('.preview-btn');

    fileRows.forEach(row => {
        row.addEventListener('click', function(e) {
            if (e.target.closest('.btn, a, button')) return;

            fileRows.forEach(r => r.classList.remove('selected'));
            row.classList.add('selected');

            showPreviewInPanel({
                id: row.dataset.fileId,
                name: row.dataset.fileName,
                fileType: row.dataset.fileType,
                fileUrl: row.dataset.fileUrl,
                previewUrl: row.dataset.previewUrl,
                fileSize: row.dataset.fileSize,
                fileUploaded: row.dataset.fileUploaded
            });
        });
    });

    previewButtons.forEach(btn => {
        btn.addEventListener('click', function(e) {
            e.preventDefault();
            e.stopPropagation();

            const row = btn.closest('.file-row');
            if (row) {
                fileRows.forEach(r => r.classList.remove('selected'));
                row.classList.add('selected');
            }

            showPreviewInPanel({
                id: btn.dataset.fileId,
                name: btn.dataset.fileName,
                fileType: btn.dataset.fileType,
                fileUrl: btn.dataset.fileUrl,
                previewUrl: btn.dataset.previewUrl,
                fileSize: btn.dataset.fileSize,
                fileUploaded: btn.dataset.fileUploaded
            });
        });
    });
}

function showPreviewInPanel(file) {
    const previewContent = document.getElementById('preview-content');
    const previewInfo = document.getElementById('preview-info');

    // Update info
    document.getElementById('info-name').textContent = file.name;
    document.getElementById('info-type').textContent = file.fileType.toUpperCase();
    document.getElementById('info-size').textContent = file.fileSize;
    document.getElementById('info-uploaded').textContent = file.fileUploaded;

    previewFileId = file.id;
    previewFileName = file.name;

    // Show loading
    previewContent.innerHTML = `
        <div class="text-center" style="padding:3rem 1rem;">
            <div class="spinner"></div>
            <p class="mt-3 small" style="color:var(--text-muted);">正在加载文件预览...</p>
        </div>
    `;
    previewInfo.style.display = 'block';

    const ft = file.fileType.toLowerCase();

    if (isImageFormat(ft)) {
        // Use the large preview image (1616x1080) for the preview box
        const url = file.previewUrl || file.fileUrl;
        const separator = url.includes('?') ? '&' : '?';
        const tokenPart = authToken ? `token=${encodeURIComponent(authToken)}` : '';

        const img = document.createElement('img');
        img.src = tokenPart ? `${url}${separator}${tokenPart}` : url;
        img.alt = file.name;
        img.className = 'preview-image';
        img.onload = function() {
            previewContent.innerHTML = '';
            previewContent.appendChild(img);
        };
        img.onerror = function() {
            previewContent.innerHTML = `
                <div class="empty-state" style="padding:2rem 1rem;">
                    <div class="empty-icon" style="font-size:3rem;color:#ef4444;">
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>
                    </div>
                    <h5 style="font-size:1rem;color:#ef4444;">预览失败</h5>
                    <p class="small">无法加载图片，文件可能不存在或格式不支持</p>
                </div>
            `;
        };
        previewContent.innerHTML = '';
        previewContent.appendChild(img);
    } else if (isVideoFormat(ft)) {
        const url = file.fileUrl;
        const separator = url.includes('?') ? '&' : '?';
        const authUrl = authToken ? `${url}${separator}token=${encodeURIComponent(authToken)}` : url;

        const video = document.createElement('video');
        video.src = authUrl;
        video.controls = true;
        video.className = 'preview-video';
        video.preload = 'metadata';
        video.onerror = function() {
            previewContent.innerHTML = `
                <div class="empty-state" style="padding:2rem 1rem;">
                    <div class="empty-icon" style="font-size:3rem;color:#ef4444;">
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>
                    </div>
                    <h5 style="font-size:1rem;color:#ef4444;">预览失败</h5>
                    <p class="small">无法播放视频，文件可能不存在或格式不支持</p>
                </div>
            `;
        };
        previewContent.innerHTML = '';
        previewContent.appendChild(video);
    } else {
        previewContent.innerHTML = `
            <div class="empty-state" style="padding:2rem 1rem;">
                <div class="empty-icon" style="font-size:3rem;">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>
                </div>
                <h5 style="font-size:1rem;">不支持预览</h5>
                <p class="small">该格式的文件无法预览，请下载后查看</p>
            </div>
        `;
    }
}

function resetPreviewPanel() {
    const previewContent = document.getElementById('preview-content');
    const previewInfo = document.getElementById('preview-info');

    if (previewContent) {
        previewContent.innerHTML = `
            <div class="empty-state" style="padding:2rem 1rem;">
                <div class="empty-icon" style="font-size:3rem;">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M15 15l-2 5L9 9l11 4-5 2zm0 0l5 5"/></svg>
                </div>
                <h4 style="font-size:1rem;">选择文件进行预览</h4>
                <p class="small">点击左侧的文件行或预览按钮可以预览图片或视频</p>
            </div>
        `;
    }
    if (previewInfo) previewInfo.style.display = 'none';
    previewFileId = null;
    previewFileName = null;
}

function getFileIconClass(fileType) {
    const ft = fileType.toLowerCase();
    if (IMAGE_FORMATS.includes(ft)) return 'image';
    if (RAW_FORMATS.includes(ft)) return 'raw';
    if (VIDEO_FORMATS.includes(ft)) return 'video';
    if (ARCHIVE_FORMATS.includes(ft)) return 'archive';
    if (DOC_FORMATS.includes(ft)) return 'doc';
    if (SPREADSHEET_FORMATS.includes(ft)) return 'spreadsheet';
    if (ft === 'pdf') return 'pdf';
    return 'default';
}

function getFileIconSvg(fileType) {
    const ft = fileType.toLowerCase();
    if (IMAGE_FORMATS.includes(ft)) return '&#128247;';
    if (RAW_FORMATS.includes(ft)) return '&#128253;';
    if (VIDEO_FORMATS.includes(ft)) return '&#127910;';
    if (ARCHIVE_FORMATS.includes(ft)) return '&#128230;';
    if (DOC_FORMATS.includes(ft)) return '&#128196;';
    if (SPREADSHEET_FORMATS.includes(ft)) return '&#128202;';
    if (ft === 'pdf') return '&#128214;';
    return '&#128196;';
}

function escHtml(str) {
    const div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
}

// ===== File Operations =====
async function deleteFile(id, name) {
    if (!confirm(`确定要删除文件 "${name}" 吗？`)) return;
    try {
        await api(`/files/${id}`, { method: 'DELETE' });
        showToast('文件已删除', 'success');
        resetPreviewPanel();
        loadHome(currentFolderId);
    } catch (err) {
        showToast(err.message, 'error');
    }
}

async function deleteFolder(id, name) {
    if (!confirm(`确定要删除文件夹 "${name}" 及其所有内容吗？`)) return;
    try {
        await api(`/folders/${id}`, { method: 'DELETE' });
        showToast('文件夹已删除', 'success');
        loadHome(currentFolderId);
    } catch (err) {
        showToast(err.message, 'error');
    }
}

function previewFile(id, name, fileType, mediaUrl, previewUrl) {
    const modal = document.getElementById('modal-overlay');
    document.getElementById('modal-title').textContent = name;
    const body = document.getElementById('modal-body');

    const ft = fileType.toLowerCase();
    const url = previewUrl || mediaUrl;
    const separator = url.includes('?') ? '&' : '?';
    const authUrl = authToken ? `${url}${separator}token=${encodeURIComponent(authToken)}` : url;

    if (isImageFormat(ft)) {
        body.innerHTML = `<img src="${authUrl}" class="preview-image" alt="${escHtml(name)}" onerror="this.parentElement.innerHTML='<p>无法加载预览</p>'">`;
    } else if (isVideoFormat(ft)) {
        body.innerHTML = `<video src="${authUrl}" class="preview-video" controls onerror="this.parentElement.innerHTML='<p>无法播放视频</p>'"></video>`;
    } else {
        body.innerHTML = '<p>该文件类型不支持预览</p>';
    }

    modal.style.display = '';
}

// ===== Upload =====
let selectedFiles = [];

async function initUploadPage() {
    // Load folders for the upload page folder selector
    try {
        const data = await api('/folders');
        const folders = data.data.folders || [];
        const select = document.getElementById('page-upload-folder');
        if (select) {
            select.innerHTML = '<option value="">根目录</option>' +
                folders.map(f => `<option value="${f.id}">${escHtml(f.name)}</option>`).join('');
        }
    } catch (err) { /* ignore */ }

    // Setup drag-drop for upload page
    const dropzone = document.getElementById('page-upload-dropzone');
    const fileInput = document.getElementById('page-file-input');
    const preview = document.getElementById('page-upload-preview');
    const uploadBtn = document.getElementById('page-upload-btn');
    const progress = document.getElementById('page-upload-progress');
    const folderSelect = document.getElementById('page-upload-folder');
    const folderName = document.getElementById('page-upload-folder-name');

    if (!dropzone || !fileInput) return;

    selectedFiles = [];

    dropzone.addEventListener('click', () => fileInput.click());
    dropzone.addEventListener('dragover', (e) => { e.preventDefault(); dropzone.classList.add('drag-over'); });
    dropzone.addEventListener('dragleave', () => dropzone.classList.remove('drag-over'));
    dropzone.addEventListener('drop', (e) => {
        e.preventDefault();
        dropzone.classList.remove('drag-over');
        addPageFiles(Array.from(e.dataTransfer.files));
    });
    fileInput.addEventListener('change', () => addPageFiles(Array.from(fileInput.files)));

    // Update folder name display
    if (folderSelect && folderName) {
        folderSelect.addEventListener('change', () => {
            const selectedOption = folderSelect.options[folderSelect.selectedIndex];
            folderName.textContent = selectedOption ? selectedOption.text : '根目录';
        });
    }

    function addPageFiles(files) {
        selectedFiles = [...selectedFiles, ...files];
        if (preview) {
            preview.innerHTML = selectedFiles.map(f => `
                <div class="upload-preview-item">
                    <span>${escHtml(f.name)}</span>
                    <small>(${formatSize(f.size)})</small>
                    <button class="remove-btn" onclick="this.parentElement.remove();selectedFiles=selectedFiles.filter(sf=>sf.name!=='${escHtml(f.name)}');document.getElementById('page-upload-btn').disabled=selectedFiles.length===0;">&times;</button>
                </div>
            `).join('');
        }
        if (uploadBtn) uploadBtn.disabled = selectedFiles.length === 0;
    }

    if (uploadBtn) {
        uploadBtn.addEventListener('click', async () => {
            if (selectedFiles.length === 0) return;
            uploadBtn.disabled = true;
            if (progress) {
                progress.style.display = '';
                progress.innerHTML = '<p>正在上传...</p>';
            }

            const folderId = folderSelect ? folderSelect.value : '';
            const formData = new FormData();
            if (folderId) formData.append('folder_id', folderId);
            selectedFiles.forEach(f => formData.append('file', f));

            try {
                const data = await api('/files/upload', {
                    method: 'POST',
                    body: formData,
                });
                const result = data.data;
                if (progress) {
                    progress.innerHTML = `<p style="color:var(--accent)">成功上传 ${result.count} 个文件</p>`;
                    if (result.errors && result.errors.length > 0) {
                        progress.innerHTML += `<p style="color:var(--danger)">${result.errors.join('<br>')}</p>`;
                    }
                }
                showToast(`成功上传 ${result.count} 个文件`, 'success');
                selectedFiles = [];
                if (preview) preview.innerHTML = '';
                if (uploadBtn) uploadBtn.disabled = true;
                setTimeout(() => { showPage('home'); }, 1000);
            } catch (err) {
                if (progress) progress.innerHTML = `<p style="color:var(--danger)">${err.message}</p>`;
                if (uploadBtn) uploadBtn.disabled = false;
            }
        });
    }
}

function showUploadModal() {
    document.getElementById('modal-title').textContent = '上传文件';
    document.getElementById('modal-body').innerHTML = `
        <div class="upload-dropzone" id="modal-upload-dropzone">
            <div class="upload-dropzone-icon">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="17 8 12 3 7 8"/></svg>
            </div>
            <p class="upload-dropzone-text">拖拽文件到此处或点击选择</p>
            <span class="upload-dropzone-hint">支持多文件上传，无文件类型和大小限制</span>
            <input type="file" class="file-input-hidden" id="modal-file-input" multiple>
        </div>
        <div id="modal-upload-preview" class="upload-preview-list"></div>
        <div class="form-group">
            <label>目标文件夹</label>
            <select id="modal-upload-folder" class="form-control">
                <option value="">根目录</option>
            </select>
        </div>
        <button class="btn btn-primary btn-block" id="modal-upload-btn" disabled>开始上传</button>
        <div id="modal-upload-progress" class="upload-progress" style="display:none;"></div>
    `;

    // Load folders for the modal
    api('/folders').then(data => {
        const folders = data.data.folders || [];
        const select = document.getElementById('modal-upload-folder');
        if (select) {
            select.innerHTML = '<option value="">根目录</option>' +
                folders.map(f => `<option value="${f.id}">${escHtml(f.name)}</option>`).join('');
        }
    }).catch(() => {});

    let modalFiles = [];

    const dropzone = document.getElementById('modal-upload-dropzone');
    const fileInput = document.getElementById('modal-file-input');
    const preview = document.getElementById('modal-upload-preview');
    const uploadBtn = document.getElementById('modal-upload-btn');
    const progress = document.getElementById('modal-upload-progress');

    dropzone.addEventListener('click', () => fileInput.click());
    dropzone.addEventListener('dragover', (e) => { e.preventDefault(); dropzone.classList.add('drag-over'); });
    dropzone.addEventListener('dragleave', () => dropzone.classList.remove('drag-over'));
    dropzone.addEventListener('drop', (e) => {
        e.preventDefault();
        dropzone.classList.remove('drag-over');
        addFiles(Array.from(e.dataTransfer.files));
    });
    fileInput.addEventListener('change', () => addFiles(Array.from(fileInput.files)));

    function addFiles(files) {
        modalFiles = [...modalFiles, ...files];
        preview.innerHTML = modalFiles.map(f => `
            <div class="upload-preview-item">
                <span>${escHtml(f.name)}</span>
                <small>(${formatSize(f.size)})</small>
                <button class="remove-btn" onclick="this.parentElement.remove();">&times;</button>
            </div>
        `).join('');
        uploadBtn.disabled = modalFiles.length === 0;
    }

    uploadBtn.addEventListener('click', async () => {
        if (modalFiles.length === 0) return;
        uploadBtn.disabled = true;
        progress.style.display = '';
        progress.innerHTML = '<p>正在上传...</p>';

        const folderId = document.getElementById('modal-upload-folder').value;
        const formData = new FormData();
        if (folderId) formData.append('folder_id', folderId);
        modalFiles.forEach(f => formData.append('file', f));

        try {
            const data = await api('/files/upload', {
                method: 'POST',
                body: formData,
            });
            const result = data.data;
            progress.innerHTML = `<p style="color:var(--accent)">成功上传 ${result.count} 个文件</p>`;
            if (result.errors && result.errors.length > 0) {
                progress.innerHTML += `<p style="color:var(--danger)">${result.errors.join('<br>')}</p>`;
            }
            showToast(`成功上传 ${result.count} 个文件`, 'success');
            setTimeout(() => { closeModal(); loadHome(currentFolderId); }, 1000);
        } catch (err) {
            progress.innerHTML = `<p style="color:var(--danger)">${err.message}</p>`;
            uploadBtn.disabled = false;
        }
    });

    document.getElementById('modal-overlay').style.display = '';
}

function formatSize(bytes) {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1048576) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1073741824) return `${(bytes / 1048576).toFixed(1)} MB`;
    return `${(bytes / 1073741824).toFixed(1)} GB`;
}

// ===== Folders =====
function showCreateFolderModal() {
    document.getElementById('modal-title').textContent = '新建文件夹';
    document.getElementById('modal-body').innerHTML = `
        <div class="form-group">
            <label>文件夹名称</label>
            <input type="text" id="new-folder-name" class="form-control" placeholder="输入文件夹名称" required>
        </div>
        <button class="btn btn-primary btn-block" onclick="createFolder()">创建</button>
    `;
    document.getElementById('modal-overlay').style.display = '';
}

async function createFolder() {
    const name = document.getElementById('new-folder-name').value.trim();
    if (!name) { showToast('请输入文件夹名称', 'error'); return; }
    try {
        await api('/folders', {
            method: 'POST',
            body: JSON.stringify({ name, parent_id: currentFolderId }),
        });
        showToast('文件夹创建成功', 'success');
        closeModal();
        loadHome(currentFolderId);
    } catch (err) {
        showToast(err.message, 'error');
    }
}

// ===== Share =====
function showShareModal(fileId, fileName) {
    document.getElementById('modal-title').textContent = `分享文件: ${fileName}`;
    document.getElementById('modal-body').innerHTML = `
        <div class="alert alert-info">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="display:inline;width:18px;height:18px;vertical-align:middle;margin-right:0.5rem;"><circle cx="12" cy="12" r="10"/><line x1="12" y1="16" x2="12" y2="12"/><line x1="12" y1="8" x2="12.01" y2="8"/></svg>
            您正在分享文件: <strong>${escHtml(fileName)}</strong>
        </div>
        <div class="form-group">
            <label>分享有效期</label>
            <select id="share-expiry" class="form-control">
                <option value="1">1小时</option>
                <option value="24">24小时</option>
                <option value="168">7天</option>
                <option value="720">30天</option>
                <option value="0">永久有效</option>
            </select>
            <div class="form-text">设置分享链接的有效期，0表示永久有效</div>
        </div>
        <div class="form-group">
            <label>访问密码（可选）</label>
            <input type="password" id="share-password" class="form-control" placeholder="留空表示无需密码">
            <div class="form-text">设置密码保护，留空表示无需密码</div>
        </div>
        <button class="btn btn-primary btn-block" onclick="createShare(${fileId})">创建分享链接</button>
    `;
    document.getElementById('modal-overlay').style.display = '';
}

async function createShare(fileId) {
    const expiresHours = parseInt(document.getElementById('share-expiry').value) || null;
    const password = document.getElementById('share-password').value || null;
    try {
        const data = await api('/shares', {
            method: 'POST',
            body: JSON.stringify({ file_id: fileId, expires_hours: expiresHours, password }),
        });
        const share = data.data;
        const url = `${window.location.origin}/share/${share.id}`;
        document.getElementById('modal-body').innerHTML = `
            <div class="alert alert-success">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="display:inline;width:18px;height:18px;vertical-align:middle;margin-right:0.5rem;"><path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"/><polyline points="22 4 12 14.01 9 11.01"/></svg>
                分享链接创建成功！
            </div>
            <div class="share-detail-list">
                <div class="share-detail-item">
                    <span class="share-detail-label">分享链接</span>
                    <span class="share-detail-value">
                        <code style="cursor:pointer;font-size:0.8rem;word-break:break-all;" onclick="navigator.clipboard.writeText('${url}');showToast('链接已复制','success')">${url}</code>
                    </span>
                </div>
                <div class="share-detail-item">
                    <span class="share-detail-label">访问密码</span>
                    <span class="share-detail-value">
                        ${share.has_password
                            ? '<span class="badge" style="background:rgba(245,158,11,0.12);color:#d97706;"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width:14px;height:14px;"><rect x="3" y="11" width="18" height="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/></svg> 已设置</span>'
                            : '<span class="badge" style="background:rgba(100,116,139,0.1);color:#64748b;"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width:14px;height:14px;"><rect x="3" y="11" width="18" height="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 9.9-1"/></svg> 无</span>'}
                    </span>
                </div>
                <div class="share-detail-item">
                    <span class="share-detail-label">过期时间</span>
                    <span class="share-detail-value">
                        ${share.expires_at ? share.expires_at : '<span style="color:#10b981;">永久有效</span>'}
                    </span>
                </div>
            </div>
            <div class="alert alert-warning mt-3">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="display:inline;width:18px;height:18px;vertical-align:middle;margin-right:0.5rem;"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>
                <strong>重要提示：</strong>请妥善保管此分享链接，任何拥有此链接的人都可以访问该文件。
            </div>
            <button class="btn btn-outline btn-block mt-2" onclick="navigator.clipboard.writeText('${url}');showToast('链接已复制','success')">复制链接</button>
        `;
    } catch (err) {
        showToast(err.message, 'error');
    }
}

async function loadShares() {
    try {
        const data = await api('/shares');
        const shares = data.data || [];
        const container = document.getElementById('shares-list');

        if (shares.length === 0) {
            container.innerHTML = `
                <div class="empty-state">
                    <div class="empty-icon">
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="18" cy="5" r="3"/><circle cx="6" cy="12" r="3"/><circle cx="18" cy="19" r="3"/><line x1="8.59" y1="13.51" x2="15.42" y2="17.49"/><line x1="15.41" y1="6.51" x2="8.59" y2="10.49"/></svg>
                    </div>
                    <h4>暂无分享文件</h4>
                    <p>您还没有创建任何文件分享链接</p>
                    <button class="btn btn-primary" onclick="showPage('home')">去上传文件并分享</button>
                </div>
            `;
            return;
        }

        container.innerHTML = `
            <div class="table-responsive" style="overflow-x:auto;">
                <table class="file-table">
                    <thead>
                        <tr>
                            <th>文件名</th>
                            <th>分享ID</th>
                            <th>创建时间</th>
                            <th>过期时间</th>
                            <th>下载次数</th>
                            <th>密码保护</th>
                            <th>操作</th>
                        </tr>
                    </thead>
                    <tbody>
                        ${shares.map(s => `
                            <tr>
                                <td>
                                    <div class="d-flex align-items-center" style="gap:0.5rem;">
                                        <div class="file-icon default" style="width:32px;height:32px;font-size:0.9rem;">
                                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width:16px;height:16px;"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>
                                        </div>
                                        ${escHtml(s.file_name)}
                                    </div>
                                </td>
                                <td><code>${s.id}</code></td>
                                <td style="font-size:0.85rem;">${s.created_at}</td>
                                <td>
                                    ${s.expires_at
                                        ? `<span style="font-size:0.85rem;">${s.expires_at}</span>`
                                        : '<span class="badge" style="background:rgba(16,185,129,0.1);color:#059669;">永久</span>'}
                                </td>
                                <td>
                                    <span class="badge" style="background:rgba(99,102,241,0.08);color:#6366f1;">
                                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width:14px;height:14px;"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/></svg>
                                        ${s.download_count}
                                    </span>
                                </td>
                                <td>
                                    ${s.has_password
                                        ? '<span class="badge" style="background:rgba(245,158,11,0.12);color:#d97706;">有</span>'
                                        : '<span class="badge" style="background:rgba(100,116,139,0.1);color:#64748b;">无</span>'}
                                </td>
                                <td style="text-align:right">
                                    <div class="file-actions" style="justify-content:flex-end">
                                        <button class="btn btn-sm btn-outline" onclick="copyShareLink('${s.id}')">复制链接</button>
                                        <button class="btn btn-sm btn-danger" onclick="deleteShare('${s.id}')">删除</button>
                                    </div>
                                </td>
                            </tr>
                        `).join('')}
                    </tbody>
                </table>
            </div>
        `;
    } catch (err) {
        showToast(err.message, 'error');
    }
}

function copyShareLink(shareId) {
    const url = `${window.location.origin}/share/${shareId}`;
    navigator.clipboard.writeText(url);
    showToast('链接已复制', 'success');
}

async function deleteShare(id) {
    if (!confirm('确定要删除这个分享链接吗？')) return;
    try {
        await api(`/shares/${id}`, { method: 'DELETE' });
        showToast('分享链接已删除', 'success');
        loadShares();
    } catch (err) {
        showToast(err.message, 'error');
    }
}

// ===== Search =====
async function loadSearchPage() {
    try {
        const data = await api('/search?q=');
        const types = data.data.file_types || [];
        const select = document.getElementById('search-type-filter');
        if (select) {
            select.innerHTML = '<option value="">所有类型</option>' +
                types.map(t => `<option value="${t}">${t.toUpperCase()}</option>`).join('');
        }
    } catch (err) { /* ignore */ }
}

async function doSearch() {
    const query = document.getElementById('search-input').value.trim();
    const fileType = document.getElementById('search-type-filter').value;
    const container = document.getElementById('search-results');
    const queryDisplay = document.getElementById('search-query-display');

    if (!query) {
        showToast('请输入搜索关键词', 'info');
        return;
    }

    try {
        const data = await api(`/search?q=${encodeURIComponent(query)}&type=${encodeURIComponent(fileType)}`);
        const result = data.data;

        if (queryDisplay) {
            queryDisplay.textContent = `"${query}"`;
        }

        let html = '';

        // Folder results
        if (result.folders && result.folders.length > 0) {
            html += `<div class="mb-4">
                <div class="search-result-title">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
                    文件夹 (${result.folders.length})
                </div>
                <div class="row" style="margin:0 -0.5rem;">
                    ${result.folders.map(f => `
                        <div style="width:50%;padding:0 0.5rem;margin-bottom:0.75rem;box-sizing:border-box;">
                            <div class="card" style="margin-bottom:0;">
                                <div class="card-body" style="padding:1rem;">
                                    <div class="d-flex align-items-center" style="gap:0.75rem;">
                                        <div class="file-icon default" style="width:44px;height:44px;font-size:1.2rem;color:#f59e0b;">
                                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
                                        </div>
                                        <div style="flex:1;min-width:0;">
                                            <h6 class="mb-1" style="font-size:0.9rem;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${escHtml(f.name)}</h6>
                                            <small style="color:var(--text-muted);">
                                                创建于 ${f.created_at || ''}
                                            </small>
                                        </div>
                                    </div>
                                    <div class="mt-3">
                                        <button class="btn btn-outline btn-sm" onclick="showPage('home',{folder_id:${f.id}})">
                                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width:14px;height:14px;"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
                                            打开文件夹
                                        </button>
                                    </div>
                                </div>
                            </div>
                        </div>
                    `).join('')}
                </div>
            </div>`;
        }

        // File results
        if (result.files && result.files.length > 0) {
            html += `<div>
                <div class="search-result-title">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>
                    文件 (${result.files.length})
                </div>
                <div class="table-responsive" style="overflow-x:auto;">
                    <table class="file-table">
                        <thead>
                            <tr><th>文件名</th><th>类型</th><th>大小</th><th>上传时间</th><th style="text-align:right">操作</th></tr>
                        </thead>
                        <tbody>
                            ${result.files.map(f => {
                                const thumbHtml = f.thumb_url
                                    ? `<img class="file-thumb" src="${f.thumb_url}&token=${encodeURIComponent(authToken || '')}" alt="" loading="lazy" onerror="this.style.display='none';this.parentElement.classList.add('${getFileIconClass(f.file_type)}');this.parentElement.innerHTML='${getFileIconSvg(f.file_type)}';">`
                                    : getFileIconSvg(f.file_type);
                                const iconClass = f.thumb_url ? '' : getFileIconClass(f.file_type);
                                return `
                                <tr>
                                    <td>
                                        <div class="file-name-cell">
                                            <div class="file-icon ${iconClass}">
                                                ${thumbHtml}
                                            </div>
                                            <div>
                                                <div class="fw-bold" style="font-size:0.9rem;">${escHtml(f.name)}</div>
                                                <small style="color:var(--text-muted);">原名: ${escHtml(f.original_name)}</small>
                                            </div>
                                        </div>
                                    </td>
                                    <td><span class="file-type-badge">${f.file_type.toUpperCase()}</span></td>
                                    <td style="color:var(--text-secondary);font-size:0.85rem;">${f.formatted_size}</td>
                                    <td style="color:var(--text-muted);font-size:0.85rem;">${f.uploaded_at}</td>
                                    <td style="text-align:right">
                                        <a class="btn btn-sm btn-outline" href="${API_BASE}/files/${f.id}/download?token=${encodeURIComponent(authToken || '')}">
                                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width:14px;height:14px;"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/></svg>
                                            下载
                                        </a>
                                    </td>
                                </tr>
                            `}).join('')}
                        </tbody>
                    </table>
                </div>
            </div>`;
        }

        if (!result.folders?.length && !result.files?.length) {
            html = `
                <div class="empty-state">
                    <div class="empty-icon">
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>
                    </div>
                    <h4>未找到匹配的结果</h4>
                    <p>请尝试其他关键词或文件类型</p>
                </div>
            `;
            if (queryDisplay) queryDisplay.textContent = '';
        }

        container.innerHTML = html;
    } catch (err) {
        showToast(err.message, 'error');
    }
}

// ===== Public Share =====
async function loadPublicShare(shareId) {
    const card = document.getElementById('share-public-card');
    try {
        const data = await api(`/public/shares/${shareId}`);
        const share = data.data;

        if (share.is_expired || !share.is_active) {
            card.innerHTML = `
                <div class="card-body">
                    <div class="empty-state">
                        <div class="empty-icon" style="color:#ef4444;">
                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>
                        </div>
                        <h3>分享已过期或失效</h3>
                        <p>该分享链接已不可用</p>
                    </div>
                </div>
            `;
            return;
        }

        if (share.has_password) {
            card.innerHTML = `
                <div class="card-header text-center">
                    <h4 class="mb-0">
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width:20px;height:20px;vertical-align:middle;margin-right:0.5rem;"><rect x="3" y="11" width="18" height="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/></svg>
                        请输入访问密码
                    </h4>
                </div>
                <div class="card-body">
                    <div class="mb-4 text-center">
                        <div class="file-icon default" style="width:64px;height:64px;font-size:1.8rem;margin:0 auto 1rem;color:var(--primary-dark);">
                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/></svg>
                        </div>
                        <h5 class="mt-3">此分享链接受密码保护</h5>
                        <p style="color:var(--text-muted);">
                            请输入密码以访问文件：<strong>${escHtml(share.file_name)}</strong>
                        </p>
                    </div>
                    <div class="form-group">
                        <label>访问密码</label>
                        <input type="password" id="share-password-input" class="form-control" placeholder="请输入密码" required>
                        <div class="form-text">分享者设置了密码保护，请输入正确的密码</div>
                    </div>
                    <button class="btn btn-primary btn-block" onclick="verifySharePassword('${shareId}')">验证密码</button>
                    <div id="share-password-error" style="color:var(--danger);margin-top:0.5rem;text-align:center"></div>
                </div>
            `;
        } else {
            showShareDownload(share);
        }
    } catch (err) {
        card.innerHTML = `
            <div class="card-body">
                <div class="empty-state">
                    <div class="empty-icon" style="color:#ef4444;">
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>
                    </div>
                    <h3>分享不存在</h3>
                    <p>${escHtml(err.message)}</p>
                </div>
            </div>
        `;
    }
}

async function verifySharePassword(shareId) {
    const password = document.getElementById('share-password-input').value;
    const errorEl = document.getElementById('share-password-error');
    errorEl.textContent = '';

    try {
        await api(`/public/shares/${shareId}/verify`, {
            method: 'POST',
            body: JSON.stringify({ password }),
        });
        sessionStorage.setItem(`share_verified_${shareId}`, 'true');
        loadPublicShare(shareId);
    } catch (err) {
        errorEl.textContent = err.message;
    }
}

function showShareDownload(share) {
    const card = document.getElementById('share-public-card');
    const ext = (share.file_type || share.file_name || '').split('.').pop() || '';
    const ft = ext.toLowerCase();

    if (isImageFormat(ft)) {
        // Image/RAW: show preview image + thumbnail
        const thumbSrc = share.thumb_url || '';
        const previewSrc = share.preview_url || '';
        card.innerHTML = `
            <div class="card-header">
                <h4 class="mb-0">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width:20px;height:20px;vertical-align:middle;margin-right:0.5rem;"><circle cx="18" cy="5" r="3"/><circle cx="6" cy="12" r="3"/><circle cx="18" cy="19" r="3"/><line x1="8.59" y1="13.51" x2="15.42" y2="17.49"/><line x1="15.41" y1="6.51" x2="8.59" y2="10.49"/></svg>
                    文件分享
                </h4>
            </div>
            <div class="share-preview-area">
                <img class="share-preview-image" src="${previewSrc}" alt="${escHtml(share.file_name)}" onerror="this.style.display='none';this.nextElementSibling.style.display='';" loading="lazy">
                <div class="share-preview-fallback" style="display:none;">
                    <div class="empty-state" style="padding:2rem;">
                        <div class="empty-icon" style="font-size:3rem;color:#ef4444;">
                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>
                        </div>
                        <p style="color:var(--text-muted);">预览加载失败</p>
                    </div>
                </div>
            </div>
            <div class="share-file-info">
                ${thumbSrc ? `<img class="share-thumb-icon" src="${thumbSrc}" alt="" onerror="this.style.display='none'">` : getFileIconSvg(ext)}
                <div class="share-file-details">
                    <h3>${escHtml(share.file_name)}</h3>
                    <p class="share-file-meta">
                        分享者: ${escHtml(share.owner_name)} |
                        大小: ${share.formatted_size || ''} |
                        ${share.expires_at ? `有效期至: ${share.expires_at}` : '永久有效'}
                    </p>
                </div>
            </div>
            <div class="share-actions">
                <a class="btn btn-primary btn-block" href="${API_BASE}/public/shares/${share.id}/download">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width:18px;height:18px;"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/></svg>
                    下载文件
                </a>
            </div>
        `;
    } else if (isVideoFormat(ft)) {
        card.innerHTML = `
            <div class="card-header">
                <h4 class="mb-0">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width:20px;height:20px;vertical-align:middle;margin-right:0.5rem;"><circle cx="18" cy="5" r="3"/><circle cx="6" cy="12" r="3"/><circle cx="18" cy="19" r="3"/><line x1="8.59" y1="13.51" x2="15.42" y2="17.49"/><line x1="15.41" y1="6.51" x2="8.59" y2="10.49"/></svg>
                    文件分享
                </h4>
            </div>
            <div class="share-preview-area">
                <video class="share-preview-video" src="${API_BASE}/public/shares/${share.id}/download" controls preload="metadata" onerror="this.style.display='none';this.nextElementSibling.style.display='';">
                    您的浏览器不支持视频播放
                </video>
                <div class="share-preview-fallback" style="display:none;">
                    <div class="empty-state" style="padding:2rem;">
                        <div class="empty-icon" style="font-size:3rem;color:#ef4444;">
                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>
                        </div>
                        <p style="color:var(--text-muted);">视频加载失败</p>
                    </div>
                </div>
            </div>
            <div class="share-file-info">
                <div class="file-icon ${getFileIconClass(ext)}">
                    ${getFileIconSvg(ext)}
                </div>
                <div class="share-file-details">
                    <h3>${escHtml(share.file_name)}</h3>
                    <p class="share-file-meta">
                        分享者: ${escHtml(share.owner_name)} |
                        大小: ${share.formatted_size || ''} |
                        ${share.expires_at ? `有效期至: ${share.expires_at}` : '永久有效'}
                    </p>
                </div>
            </div>
            <div class="share-actions">
                <a class="btn btn-primary btn-block" href="${API_BASE}/public/shares/${share.id}/download">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width:18px;height:18px;"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/></svg>
                    下载文件
                </a>
            </div>
        `;
    } else {
        // Other file types: show icon + download button
        card.innerHTML = `
            <div class="card-header">
                <h4 class="mb-0">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width:20px;height:20px;vertical-align:middle;margin-right:0.5rem;"><circle cx="18" cy="5" r="3"/><circle cx="6" cy="12" r="3"/><circle cx="18" cy="19" r="3"/><line x1="8.59" y1="13.51" x2="15.42" y2="17.49"/><line x1="15.41" y1="6.51" x2="8.59" y2="10.49"/></svg>
                    文件分享
                </h4>
            </div>
            <div class="share-public-info">
                ${share.thumb_url ? `<img class="share-thumb-icon" src="${share.thumb_url}" alt="" style="width:64px;height:64px;margin:0 auto 1rem;display:block;border-radius:8px;object-fit:cover;" onerror="this.style.display='none'">` : `<div class="file-icon ${getFileIconClass(ext)}" style="width:64px;height:64px;font-size:2rem;margin:0 auto 1rem;">${getFileIconSvg(ext)}</div>`}
                <h3>${escHtml(share.file_name)}</h3>
                <p class="file-meta">
                    分享者: ${escHtml(share.owner_name)} |
                    大小: ${share.formatted_size || ''} |
                    ${share.expires_at ? `有效期至: ${share.expires_at}` : '永久有效'}
                </p>
                <a class="btn btn-primary btn-block" href="${API_BASE}/public/shares/${share.id}/download" style="margin-top:1rem">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width:18px;height:18px;"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/></svg>
                    下载文件
                </a>
            </div>
        `;
    }
}

// ===== Modal =====
function closeModal() {
    document.getElementById('modal-overlay').style.display = 'none';
}

// ===== Check for share route on load =====
(function checkShareRoute() {
    if (window.location.pathname.startsWith('/share/')) {
        const shareId = window.location.pathname.split('/share/')[1];
        if (shareId) {
            document.querySelectorAll('.page').forEach(p => p.style.display = 'none');
            document.getElementById('page-share-public').style.display = '';
            document.getElementById('nav-links').style.display = 'none';
            document.getElementById('nav-user').style.display = 'none';
            loadPublicShare(shareId);
            return true;
        }
    }
    return false;
})();

// ===== Nav click handlers =====
document.querySelectorAll('.nav-link').forEach(link => {
    link.addEventListener('click', (e) => {
        e.preventDefault();
        showPage(link.dataset.page);
    });
});

// ===== Batch Operations =====
function onItemCheckbox(cb) {
    const key = cb.dataset.key;
    if (cb.checked) {
        selectedItems.set(key, {
            id: parseInt(cb.dataset.id),
            name: cb.dataset.name,
            type: cb.dataset.type
        });
    } else {
        selectedItems.delete(key);
    }
    updateBatchBar();
}

function toggleSelectAll(masterCb) {
    const checkboxes = document.querySelectorAll('.file-checkbox');
    checkboxes.forEach(cb => {
        cb.checked = masterCb.checked;
        const key = cb.dataset.key;
        if (masterCb.checked) {
            selectedItems.set(key, {
                id: parseInt(cb.dataset.id),
                name: cb.dataset.name,
                type: cb.dataset.type
            });
        } else {
            selectedItems.delete(key);
        }
    });
    updateBatchBar();
}

function updateBatchBar() {
    const bar = document.getElementById('batch-bar');
    const count = document.getElementById('batch-count');
    const selectAll = document.getElementById('batch-select-all');
    if (!bar || !count) return;

    const total = selectedItems.size;
    if (total > 0) {
        bar.style.display = '';
        count.textContent = `已选 ${total} 项`;
        // Sync select-all checkbox
        if (selectAll) {
            const allCbs = document.querySelectorAll('.file-checkbox');
            selectAll.checked = allCbs.length > 0 && total === allCbs.length;
            selectAll.indeterminate = total > 0 && total < allCbs.length;
        }
    } else {
        bar.style.display = 'none';
        if (selectAll) selectAll.checked = false;
    }
}

function getSelectedIds() {
    const fileIds = [];
    const folderIds = [];
    selectedItems.forEach((item, key) => {
        if (item.type === 'file') fileIds.push(item.id);
        else if (item.type === 'folder') folderIds.push(item.id);
    });
    return { fileIds, folderIds };
}

function showBatchMoveDialog() {
    const { fileIds, folderIds } = getSelectedIds();
    if (fileIds.length === 0 && folderIds.length === 0) {
        showToast('请先选择文件或文件夹', 'warning');
        return;
    }
    showBatchTargetDialog('move', fileIds, folderIds);
}

function showBatchCopyDialog() {
    const { fileIds, folderIds } = getSelectedIds();
    if (fileIds.length === 0 && folderIds.length === 0) {
        showToast('请先选择文件', 'warning');
        return;
    }
    if (fileIds.length === 0) {
        showToast('复制操作暂不支持文件夹，请选择文件', 'warning');
        return;
    }
    // Only pass file IDs for copy; warn if folders were also selected
    if (folderIds.length > 0) {
        showToast(`已自动排除 ${folderIds.length} 个文件夹（复制暂不支持文件夹）`, 'warning');
    }
    showBatchTargetDialog('copy', fileIds, []);
}

async function showBatchTargetDialog(action, fileIds, folderIds) {
    const overlay = document.getElementById('modal-overlay');
    const title = document.getElementById('modal-title');
    const body = document.getElementById('modal-body');

    title.textContent = action === 'move' ? '移动到...' : '复制到...';

    // Load folder tree
    let foldersHtml = '';
    try {
        const data = await api('/folders');
        const folders = data.data.folders || [];
        foldersHtml = `
            <div class="folder-tree">
                <div class="folder-tree-item" data-folder-id="" onclick="selectBatchTarget(this, null)">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" style="width:18px;height:18px;"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
                    <span>根目录</span>
                </div>
                ${folders.map(f => `
                    <div class="folder-tree-item" data-folder-id="${f.id}" onclick="selectBatchTarget(this, ${f.id})">
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" style="width:18px;height:18px;"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
                        <span>${escHtml(f.name)}</span>
                    </div>
                `).join('')}
            </div>
        `;
    } catch (err) {
        foldersHtml = `<p style="color:var(--text-muted);padding:1rem;">加载文件夹列表失败</p>`;
    }

    body.innerHTML = `
        <div style="margin-bottom:1rem;">
            <label style="font-weight:600;font-size:0.9rem;display:block;margin-bottom:0.5rem;">选择目标文件夹</label>
            ${foldersHtml}
        </div>
        <div style="margin-bottom:1rem;">
            <label style="font-weight:600;font-size:0.9rem;display:block;margin-bottom:0.5rem;">冲突处理</label>
            <div style="display:flex;flex-direction:column;gap:0.5rem;">
                <label class="radio-label">
                    <input type="radio" name="conflict" value="rename" checked> 自动重命名（添加序号）
                </label>
                <label class="radio-label">
                    <input type="radio" name="conflict" value="skip"> 跳过已存在文件
                </label>
                <label class="radio-label">
                    <input type="radio" name="conflict" value="overwrite"> 覆盖已存在文件
                </label>
            </div>
        </div>
        <div style="display:flex;gap:0.5rem;justify-content:flex-end;margin-top:1rem;">
            <button class="btn btn-outline" onclick="closeModal()">取消</button>
            <button class="btn btn-primary" id="confirm-batch-btn" disabled onclick="confirmBatchAction()">确认${action === 'move' ? '移动' : '复制'}</button>
        </div>
    `;

    window._batchAction = action;
    window._batchFileIds = fileIds;
    window._batchFolderIds = folderIds;
    window._batchTargetId = null;

    overlay.style.display = '';
}

function selectBatchTarget(el, folderId) {
    document.querySelectorAll('.folder-tree-item').forEach(i => i.classList.remove('selected'));
    el.classList.add('selected');
    window._batchTargetId = folderId;
    document.getElementById('confirm-batch-btn').disabled = false;
}

async function confirmBatchAction() {
    const action = window._batchAction;
    const targetId = window._batchTargetId;
    const fileIds = window._batchFileIds;
    const folderIds = window._batchFolderIds;
    const conflictEl = document.querySelector('input[name="conflict"]:checked');
    const conflict = conflictEl ? conflictEl.value : 'rename';

    if (targetId === undefined) {
        showToast('请选择目标文件夹', 'warning');
        return;
    }

    closeModal();

    const endpoint = action === 'move' ? '/batch/move' : '/batch/copy';
    const actionName = action === 'move' ? '移动' : '复制';

    try {
        showToast(`正在${actionName}...`, 'info');
        const data = await api(endpoint, {
            method: 'POST',
            body: JSON.stringify({
                file_ids: fileIds,
                folder_ids: folderIds,
                target_folder_id: targetId,
                conflict_strategy: conflict,
            }),
        });
        const result = data.data;
        const parts = [`成功${actionName} ${result.succeeded} 项`];
        if (result.skipped > 0) parts.push(`跳过 ${result.skipped} 项`);
        if (result.failed > 0) parts.push(`失败 ${result.failed} 项`);
        showToast(parts.join('，'), result.failed > 0 ? 'warning' : 'success');

        clearSelection();
        loadHome(currentFolderId);
    } catch (err) {
        showToast(`${actionName}失败: ${err.message}`, 'error');
    }
}

async function batchDeleteFiles() {
    const { fileIds, folderIds } = getSelectedIds();
    if (fileIds.length === 0 && folderIds.length === 0) {
        showToast('请先选择文件或文件夹', 'warning');
        return;
    }

    const total = fileIds.length + folderIds.length;
    if (!confirm(`确定要删除选中的 ${total} 项吗？此操作不可撤销。`)) return;

    try {
        showToast('正在删除...', 'info');
        const data = await api('/batch/delete', {
            method: 'POST',
            body: JSON.stringify({ file_ids: fileIds, folder_ids: folderIds }),
        });
        const result = data.data;
        const parts = [`成功删除 ${result.deleted} 项`];
        if (result.failed > 0) parts.push(`失败 ${result.failed} 项`);
        showToast(parts.join('，'), result.failed > 0 ? 'warning' : 'success');

        clearSelection();
        loadHome(currentFolderId);
    } catch (err) {
        showToast(`删除失败: ${err.message}`, 'error');
    }
}

function showBatchShareDialog() {
    const { fileIds } = getSelectedIds();
    if (fileIds.length === 0) {
        showToast('请先选择文件', 'warning');
        return;
    }

    const overlay = document.getElementById('modal-overlay');
    const title = document.getElementById('modal-title');
    const body = document.getElementById('modal-body');

    title.textContent = '批量分享';
    body.innerHTML = `
        <div style="margin-bottom:1rem;">
            <p style="color:var(--text-secondary);font-size:0.9rem;">将为选中的 ${fileIds.length} 个文件创建独立的分享链接</p>
        </div>
        <div class="form-group">
            <label>有效期（小时）</label>
            <input type="number" class="form-control" id="batch-share-hours" placeholder="留空为永久有效" min="1" value="24">
        </div>
        <div class="form-group">
            <label>访问密码（可选）</label>
            <input type="text" class="form-control" id="batch-share-password" placeholder="留空为无密码">
        </div>
        <div style="display:flex;gap:0.5rem;justify-content:flex-end;margin-top:1rem;">
            <button class="btn btn-outline" onclick="closeModal()">取消</button>
            <button class="btn btn-primary" onclick="confirmBatchShare(${JSON.stringify(fileIds).replace(/"/g, '&quot;')})">确认分享</button>
        </div>
    `;

    overlay.style.display = '';
}

async function confirmBatchShare(fileIds) {
    const hoursVal = document.getElementById('batch-share-hours').value;
    const password = document.getElementById('batch-share-password').value;
    const expiresHours = hoursVal ? parseInt(hoursVal) : null;

    closeModal();

    try {
        showToast('正在创建分享链接...', 'info');
        const data = await api('/batch/share', {
            method: 'POST',
            body: JSON.stringify({
                file_ids: fileIds,
                expires_hours: expiresHours,
                password: password || null,
            }),
        });
        const result = data.data;
        showToast(`成功创建 ${result.succeeded} 个分享链接`, 'success');

        clearSelection();
        loadHome(currentFolderId);
    } catch (err) {
        showToast(`分享失败: ${err.message}`, 'error');
    }
}

function clearSelection() {
    selectedItems.clear();
    updateBatchBar();
}