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
echo 无法启动 Windows PowerShell。请检查或修复 Windows PowerShell 系统组件。
echo 轻量看盘 v2 为 Windows x64 原生程序，无需安装 .NET Framework 或 VC++ 运行库。
echo 也可从官方 Releases 下载 LiteTicker-win.zip，解压后直接运行 LiteTicker.exe。
echo https://github.com/Alexzzz2008/LiteTicker/releases/latest
pause
exit /b 1
