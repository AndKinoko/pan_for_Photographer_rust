@echo off
chcp 65001 >nul
cd /d "%~dp0"
setlocal

rem ===========================================================================
rem  docker_start_server.bat  --  容器化部署与运维
rem ---------------------------------------------------------------------------
rem  用法：
rem    双击运行        交互式菜单，选 1-9
rem    带参数运行      docker_start_server.bat up
rem
rem  命令：
rem    up       启动        构建镜像并启动（首次会自动生成配置与密钥）
rem    dev      本机调试    额外开 127.0.0.1:8000 入口，浏览器能访问
rem    down     停止        停止并移除容器，保留数据卷
rem    logs     查看日志    跟踪全部容器输出
rem    rebuild  重新构建    改了代码后重新构建镜像
rem    backup   备份        备份数据库与上传文件
rem    status   查看状态    容器状态与健康检查
rem    prune    清理资源    删除停止的容器与悬空镜像（不动数据卷）
rem    undev    收回调试    关闭本机调试入口
rem ===========================================================================

set "CMD=%~1"
set "COMPOSE=docker compose --env-file deploy.env"

rem ---------------------------------------------------------------------------
rem  统一收尾。**任何路径退出前都先 pause** ——
rem  双击运行时窗口不会闪退，看得见错误信息。
rem  设了 NO_PAUSE 的分支是「用户已经看过、正在连续操作」，不再拦一次。
rem ---------------------------------------------------------------------------
:done
if not defined NO_PAUSE pause
if defined ERRLVL exit /b %ERRLVL%
exit /b 0

rem ---------------------------------------------------------------------------
rem  前置检查
rem ---------------------------------------------------------------------------

rem Docker 引擎是否在跑。命令行工具装好 != 引擎在跑。
docker info >nul 2>&1
if errorlevel 1 goto :no_docker

rem deploy.env 必须存在。不存在就从模板复制，然后停下让用户去改 ——
rem 继续跑只会在 compose 那里报一堆看不懂的错。
if not exist "deploy.env" (
    echo [i] 首次运行：正在从 deploy.env.example 生成 deploy.env ...
    copy /y "deploy.env.example" "deploy.env" >nul
    echo.
    echo [v] 已生成 deploy.env
    echo.
    echo     继续之前，请用记事本打开 deploy.env，至少确认这两项：
    echo.
    echo       SEED_ADMIN_PASSWORD=...    留空则不创建管理员，无法登录管理端
    echo       CORS_ALLOWED_ORIGINS=...   填自己的域名，公网部署必填
    echo.
    echo     改完保存，再运行本脚本。
    goto :done
)

rem TUNNEL_ID：compose.yaml 里是 ${TUNNEL_ID:?...}，缺它时 compose 在**解析
rem 变量阶段**就失败 —— 连 app 服务都起不来，down 也会被拦。
rem 这里显式设占位值保证本地命令始终可用；不填真实 ID 时不会起隧道容器。
set "TUNNEL_ID="
for /f "tokens=1,* delims==" %%A in ('findstr /b "TUNNEL_ID=" deploy.env 2^>nul') do set "TUNNEL_ID=%%B"
if not defined TUNNEL_ID set "TUNNEL_ID=local-only"
set "HAS_TUNNEL=0"
if not "%TUNNEL_ID%"=="local-only" set "HAS_TUNNEL=1"

rem ---------------------------------------------------------------------------
rem  菜单
rem ---------------------------------------------------------------------------
if defined CMD goto :run

:menu
echo.
echo   ============================================
echo    摄影师网盘 - 容器化部署
echo   ============================================
if "%HAS_TUNNEL%"=="1" (
    echo    当前模式：正式部署（已配置 Cloudflare 隧道）
) else (
    echo    当前模式：本机部署（未配置 Cloudflare 隧道）
)
echo.
echo    1  启动        构建镜像并启动服务
echo    2  本机调试    额外开 127.0.0.1:8000，浏览器可访问
echo    3  停止        停止容器，保留数据
echo    4  查看日志    跟踪输出
echo    5  重新构建    改了代码后重新构建镜像
echo    6  备份        备份数据库与上传文件
echo    7  查看状态    容器状态与健康检查
echo    8  清理资源    删除停止的容器与悬空镜像
echo    9  收回调试    关闭 127.0.0.1 入口
echo.
set "PICK="
set /p "PICK=请选择（直接回车 = 1 启动）: "
if not defined PICK set "PICK=1"
if /i "%PICK%"=="1" set "CMD=up"
if /i "%PICK%"=="2" set "CMD=dev"
if /i "%PICK%"=="3" set "CMD=down"
if /i "%PICK%"=="4" set "CMD=logs"
if /i "%PICK%"=="5" set "CMD=rebuild"
if /i "%PICK%"=="6" set "CMD=backup"
if /i "%PICK%"=="7" set "CMD=status"
if /i "%PICK%"=="8" set "CMD=prune"
if /i "%PICK%"=="9" set "CMD=undev"
if not defined CMD (
    echo 无效的选择。
    goto :done
)

:run
if /i "%CMD%"=="up" goto :cmd_up
if /i "%CMD%"=="dev" goto :cmd_dev
if /i "%CMD%"=="down" goto :cmd_down
if /i "%CMD%"=="logs" goto :cmd_logs
if /i "%CMD%"=="rebuild" goto :cmd_rebuild
if /i "%CMD%"=="backup" goto :cmd_backup
if /i "%CMD%"=="status" goto :cmd_status
if /i "%CMD%"=="prune" goto :cmd_prune
if /i "%CMD%"=="undev" goto :cmd_undev
if /i "%CMD%"=="help" goto :cmd_help
echo 未知的命令：%CMD%
set "NO_PAUSE=1"
goto :menu

:cmd_help
echo 可用命令：up dev down logs rebuild backup status prune undev
set "NO_PAUSE=1"
goto :menu

rem ---------------------------------------------------------------------------
:no_docker
echo [x] Docker 引擎未运行。
echo.
echo     请先启动 Docker Desktop，等托盘图标变成绿色再运行本脚本。
echo     若已启动仍报此错，在管理员 PowerShell 里执行 wsl --shutdown，
echo     然后重新打开 Docker Desktop。
echo.
goto :done

rem ===========================================================================
rem  准备 secret.key
rem ---------------------------------------------------------------------------
rem  config.rs 在密钥缺失时直接 panic，容器会进 restart 循环刷屏日志。
rem
rem  必须用一次性容器写进 named volume —— 宿主机上随便生成一个文件
rem  **不会**出现在容器里（compose 挂的是卷，不是 bind mount）。
rem ---------------------------------------------------------------------------
:ensure_secret
%COMPOSE% run --rm --no-deps --entrypoint sh app -c "[ -f /data/secret.key ] || (head -c 48 /dev/urandom | base64 > /data/secret.key && chmod 600 /data/secret.key)" >nul 2>&1
if errorlevel 1 (
    echo [x] 准备 secret.key 失败，通常是镜像还不存在。
    echo     请选择「5 重新构建」后再启动。
    set "ERRLVL=1"
)
goto :eof

rem ===========================================================================
rem  1. 启动
rem ===========================================================================
:cmd_up
echo.
echo [1/3] 准备密钥 ...
call :ensure_secret
if errorlevel 1 goto :done

echo [2/3] 构建镜像并启动 ...
if "%HAS_TUNNEL%"=="1" (
    echo       模式：正式部署（应用 + Cloudflare 隧道）
    %COMPOSE% up -d --build
) else (
    echo       模式：本机部署（仅应用，未配置隧道）
    %COMPOSE% up -d --build app
)
if errorlevel 1 (
    echo.
    echo [x] 启动失败。可按提示排查：
    echo.
    echo     提到 TUNNEL_ID  -> 编辑 deploy.env 填入隧道 ID
    echo     提到镜像不存在  -> 选择「5 重新构建」
    echo     其它错误       -> 选择「4 查看日志」看完整输出
    set "ERRLVL=1"
    goto :done
)

echo [3/3] 当前状态 ...
echo.
%COMPOSE% ps
echo.
echo [v] 启动完成。
echo.
if "%HAS_TUNNEL%"=="1" (
    echo     访问方式：通过你的 Cloudflare 域名。
    echo     宿主机上没有对外端口，这是有意的 —— 外部只能经隧道进来。
) else (
    echo     当前没有对外入口，浏览器打不开是正常的。
    echo     想在浏览器里看看，选择「2 本机调试」。
    echo     正式部署请编辑 deploy.env 填入 TUNNEL_ID 后重新启动。
)
goto :done

rem ===========================================================================
rem  2. 本机调试
rem ---------------------------------------------------------------------------
rem  用 compose profile（app-dev）而不是给 app 加 ports：端口映射写在 app
rem  上就成了常驻入口，撤掉还得改文件；做成 profile 后 rm 就彻底消失。
rem ---------------------------------------------------------------------------
:cmd_dev
echo.
echo [1/2] 准备密钥 ...
call :ensure_secret
if errorlevel 1 goto :done

echo [2/2] 启动本机调试入口 ...
%COMPOSE% --profile dev up -d app-dev
if errorlevel 1 (
    echo [x] 启动失败
    set "ERRLVL=1"
    goto :done
)
echo.
echo [v] 已启动，浏览器打开： http://127.0.0.1:8000
echo.
echo     这个入口只绑 127.0.0.1，局域网和公网都访问不到。
echo     排查完请选择「9 收回调试」—— 留着等于在防火墙上开了个本机洞。
goto :done

rem ===========================================================================
rem  9. 收回调试入口
rem ===========================================================================
:cmd_undev
echo.
echo 正在收回 127.0.0.1 调试入口 ...
%COMPOSE% --profile dev rm -sf app-dev
if errorlevel 1 (
    echo [!] 收回失败。可手动执行：
    echo       docker compose --env-file deploy.env --profile dev rm -sf app-dev
    goto :done
)
echo [v] 已收回。宿主机的 8000 端口现在又访问不到了。
goto :done

rem ===========================================================================
rem  3. 停止
rem ---------------------------------------------------------------------------
rem  刻意不加 -v：删卷等于删库删照片，不可逆。
rem ---------------------------------------------------------------------------
:cmd_down
echo.
echo 正在停止服务 ...
echo （数据卷 pan-data 会保留，只有手动删除卷才会丢）
echo.
%COMPOSE% down
if errorlevel 1 (
    echo [x] 停止失败
    set "ERRLVL=1"
    goto :done
)
echo [v] 已停止。
goto :done

rem ===========================================================================
rem  5. 重新构建镜像
rem ---------------------------------------------------------------------------
rem  日常改代码用默认缓存即可（改了源码的那一层会自动失效）。
rem  真要 --no-cache 时 200 多个依赖 crate 全部重编，十几分钟。
rem ---------------------------------------------------------------------------
:cmd_rebuild
echo.
echo 正在重新构建镜像 ...
echo （首次或依赖变更后可能数分钟；日常改代码命中缓存很快）
echo.
docker build -t pan-for-photographer:latest .
if errorlevel 1 (
    echo [x] 构建失败
    set "ERRLVL=1"
    goto :done
)
echo.
echo [v] 构建完成。选择「1 启动」把它应用到容器。
goto :done

rem ===========================================================================
rem  4. 日志
rem ===========================================================================
:cmd_logs
echo.
echo 跟踪日志中，按 Ctrl+C 退出。
echo.
%COMPOSE% logs -f
set "NO_PAUSE=1"
goto :menu

rem ===========================================================================
rem  6. 备份
rem ---------------------------------------------------------------------------
rem  必须用 sqlite3 的 .backup，不能 cp data.db：
rem  WAL 模式下已提交数据可能还在 -wal 里，直接 cp 会得到空壳，
rem  或者更危险的「看着正常但缺最近提交」的副本。
rem ---------------------------------------------------------------------------
:cmd_backup
echo.
echo 正在备份 ...
echo.
%COMPOSE% exec -T app sh /app/backup.sh
if errorlevel 1 (
    echo [x] 备份失败。服务是否在运行？请先选择「1 启动」。
    set "ERRLVL=1"
    goto :done
)
echo.
echo [v] 备份位于容器卷内 /data/backups。取回宿主机执行：
echo       docker compose --env-file deploy.env cp app:/data/backups .\backups
echo.
echo     data.db、uploads、secret.key 三者必须一起保管，缺一不可。
goto :done

rem ===========================================================================
rem  7. 状态
rem ===========================================================================
:cmd_status
echo.
%COMPOSE% ps
echo.
echo 健康检查（容器内部）：
%COMPOSE% exec -T app curl -fsS http://127.0.0.1:8000/api/health
echo.
goto :done

rem ===========================================================================
rem  8. 清理资源
rem ---------------------------------------------------------------------------
rem  只删停止的容器与悬空镜像，**不碰数据卷**。
rem ---------------------------------------------------------------------------
:cmd_prune
echo.
echo 正在清理停止的容器与悬空镜像 ...
echo （不会删除数据卷 pan-data）
echo.
docker container prune -f
docker image prune -f
echo.
echo [v] 清理完成。
goto :done
