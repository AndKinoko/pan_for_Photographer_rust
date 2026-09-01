@echo off
chcp 65001 >nul
cd /d "%~dp0"

:: ─── 网络绑定配置 ─────────────────────────────────────────────────
:: 留空则使用默认值（0.0.0.0 仅 IPv4）
:: IPv6 双栈（同时监听 IPv4 + IPv6）：set SERVER_HOST=::
:: 指定 IPv6 地址：              set SERVER_HOST=2001:db8::1
:: IPv4 地址：                   set SERVER_HOST=0.0.0.0
:: ─────────────────────────────────────────────────────────────────
if not defined SERVER_HOST set SERVER_HOST=::
if not defined SERVER_PORT set SERVER_PORT=0100

echo SERVER_HOST=%SERVER_HOST%  SERVER_PORT=%SERVER_PORT%
echo Starting Pan For Photographer...

cargo run --release
pause