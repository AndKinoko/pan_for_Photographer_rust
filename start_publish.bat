@echo off
title Photographer-Pan-Startup
cd /d "%~dp0"

echo ============================================================
echo   Photographer Netdisk - Dual-port Launcher
echo ------------------------------------------------------------
echo   [8001] Normal User Frontend : login / browse / preview / download
echo   [8002] Admin Frontend       : super admin AKIHANA manage users
echo                                (expiry, password, batch upload orig)
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
echo [Step 2/2] Starting two port services ...
echo.

start "PUBLISH-USER-8001" cmd /k "set SERVER_HOST=0.0.0.0& set SERVER_PORT=8001& set STATIC_DIR=static_user& set UPLOAD_DIR=./uploads& set DATABASE_PATH=./data.db& cargo run --release"
start "PUBLISH-ADMIN-8002" cmd /k "set SERVER_HOST=0.0.0.0& set SERVER_PORT=8002& set STATIC_DIR=static_admin& set UPLOAD_DIR=./uploads& set DATABASE_PATH=./data.db& cargo run --release"

echo.
echo  Launch commands sent. Two service windows are opening ...
echo    Normal user : http://localhost:8001
echo    Admin       : http://localhost:8002
echo    Super admin : AKIHANA / ljyljy
echo.
pause