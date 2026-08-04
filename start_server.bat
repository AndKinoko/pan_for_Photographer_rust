@echo off
chcp 65001 >nul
cd /d "%~dp0"
echo Starting PAN FOR PHOTOGRAPHER Server...
cargo run --release
pause