@echo off
title Photographer-Pan-Delivery
cd /d "%~dp0"

echo ============================================================
echo   Photographer Netdisk - Dual-port Launcher
echo ------------------------------------------------------------
echo   [8001] Delivery Frontend (static_user)
echo          客户只读交付端：登录 / 浏览 / 预览 / 下载（保留）
echo   [8002] Unified Vue Frontend (static)
echo          完整功能 + 管理端 /admin（用户管理 / 有效期 / 上传原图）
echo          旧的管理端独立前端 static_admin 已废弃（并入 Vue）
echo ============================================================
echo.

echo [Step 1/2] Building backend (release) ...
cargo build --release
if errorlevel 1 (
    echo.
    echo BUILD FAILED. Check Rust toolchain and source code.
    pause
    exit /b 1
)

echo.
echo [Step 2/2] Starting two services ...
echo.

start "DELIVERY-8001" cmd /k "set SERVER_HOST=0.0.0.0& set SERVER_PORT=8001& set STATIC_DIR=static_user& set UPLOAD_DIR=./uploads& set DATABASE_PATH=./data.db& cargo run --release"
start "PAN-UNIFIED-8002" cmd /k "set SERVER_HOST=0.0.0.0& set SERVER_PORT=8002& set STATIC_DIR=static& set UPLOAD_DIR=./uploads& set DATABASE_PATH=./data.db& cargo run --release"

echo.
echo  Launch commands sent. Two service windows are opening ...
echo    Delivery (客户取片) : http://localhost:8001
echo    超级管理员 : 由 SEED_ADMIN_USERNAME / SEED_ADMIN_PASSWORD 环境变量控制（首次启动时设置）
echo.
pause