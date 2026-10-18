@echo off
@rem ===========================================================================
@rem  Echolet Windows Diagnostics Doctor (Launcher)
@rem ===========================================================================
@rem  This wrapper launches doctor.ps1 using Windows PowerShell with a process-scoped
@rem  execution policy bypass (-ExecutionPolicy Bypass).
@rem
@rem  Safety contract:
@rem  - Runs read-only diagnostics without modifying machine configuration or registry.
@rem  - Does not change the persistent system-wide PowerShell execution policy.
@rem  - Does not require administrator privileges.
@rem  - Locates bundled files relative to this script, regardless of caller directory.
@rem ===========================================================================

setlocal EnableExtensions
set "SCRIPT_DIR=%~dp0"

powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT_DIR%doctor.ps1" %*
set "DOCTOR_EXIT=%ERRORLEVEL%"

@rem If launched by double-clicking in Windows Explorer, pause so the user can read the output.
echo %cmdcmdline% | findstr /i /c:"%~nx0" >nul 2>&1
if not errorlevel 1 (
    echo %cmdcmdline% | findstr /i /c:"/c" >nul 2>&1
    if not errorlevel 1 (
        echo.
        echo Press any key to exit . . .
        pause >nul
    )
)

exit /b %DOCTOR_EXIT%
