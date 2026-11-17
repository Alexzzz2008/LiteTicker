@echo off
setlocal DisableDelayedExpansion
"%SystemRoot%\System32\chcp.com" 65001 >nul
set "LITETICKER_POWERSHELL=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"
if not exist "%LITETICKER_POWERSHELL%" goto powershell_unavailable
"%LITETICKER_POWERSHELL%" -NoProfile -NonInteractive -Command "exit 0" >nul 2>&1
if not "%ERRORLEVEL%"=="0" goto powershell_unavailable

"%LITETICKER_POWERSHELL%" -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" -Start %*
set "LITETICKER_INSTALL_EXIT=%ERRORLEVEL%"
if "%LITETICKER_INSTALL_EXIT%"=="0" exit /b 0
echo.
echo 安装未完成。请查看上方具体错误，处理后重新运行 install.cmd。
pause
exit /b %LITETICKER_INSTALL_EXIT%

:powershell_unavailable
echo 无法启动 Windows PowerShell。请先检查 Windows PowerShell 或系统组件是否可用。
echo 如果系统提示缺少 .NET Framework，请从微软官网下载 .NET Framework 4.8 或更新版本：
echo https://dotnet.microsoft.com/zh-cn/download/dotnet-framework/net48
set "LITETICKER_OPEN_PAGE="
set /p "LITETICKER_OPEN_PAGE=输入 Y 打开微软官方下载页面，直接回车取消："
if /i "%LITETICKER_OPEN_PAGE%"=="Y" start "" "https://dotnet.microsoft.com/zh-cn/download/dotnet-framework/net48"
echo 安装未开始。修复系统组件或安装运行库后，请重新运行 install.cmd。
pause
exit /b 1
