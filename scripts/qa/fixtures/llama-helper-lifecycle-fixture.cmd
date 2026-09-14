@echo off
setlocal EnableExtensions DisableDelayedExpansion
set "MODE=%~1"

if /I "%MODE%"=="ignore-shutdown" (
  start "" /b "%SystemRoot%\System32\ping.exe" -n 120 127.0.0.1 ^>nul
  goto :hold
)
if /I "%MODE%"=="parent-exit" (
  start "" /b "%SystemRoot%\System32\ping.exe" -n 120 127.0.0.1 ^>nul
  goto :hold
)

:read
set "LINE="
set /p "LINE="
if errorlevel 1 goto :done

if not "%LINE:shutdown=%"=="%LINE%" goto :shutdown
if not "%LINE:ping=%"=="%LINE%" goto :ping
if not "%LINE:generate=%"=="%LINE%" goto :generate
goto :read

:shutdown
if /I "%MODE%"=="delayed-shutdown" "%SystemRoot%\System32\ping.exe" -n 2 127.0.0.1 >nul
if /I "%MODE%"=="ignore-shutdown" goto :read
if /I "%MODE%"=="parent-exit" goto :read
echo({"type":"goodbye"}
goto :done

:ping
echo({"type":"pong"}
goto :read

:generate
if /I "%MODE%"=="failure" (
  echo({"type":"response","text":"","error":"fixture model failure"}
) else (
  echo({"type":"response","text":"generated text","error":null}
)
goto :read

:hold
"%SystemRoot%\System32\ping.exe" -n 120 127.0.0.1 >nul
goto :hold

:done
endlocal
