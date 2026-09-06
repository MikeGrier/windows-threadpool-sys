@echo off
rem Copyright (c) Mike Grier. All rights reserved.
rem
rem The "cargo" for sabotage2.json: makes the sabotage harness's own test suite
rem look like a cargo test run, so the harness can be pointed at itself.
rem
rem Passed to run-sabotage.ps1 as -CargoCommand. Two things it must absorb:
rem
rem   --no-run      The build phase. There is nothing to compile, so it succeeds
rem                 immediately. Answering it here rather than teaching the
rem                 harness about non-cargo runners is what lets self-sabotage
rem                 work with no change to the harness at all.
rem
rem   --target-dir  The harness always aims builds at its working copy's own
rem                 target directory. Meaningless here, and simply ignored.
rem
rem The harness runs this with the working directory set to its COPY of the
rem tree, so the suite invoked below is the copy's -- which tests the copy's
rem run-sabotage.ps1, the one carrying the injected defect. That is the whole
rem trick: the sabotage lands in the copy, and the copy's tests are what judge
rem it.
rem
rem No recursion hazard: the inner suite builds its own throwaway fixtures under
rem TEMP, never inside this copy.

echo %* | findstr /C:"--no-run" >nul && exit /b 0

rem PowerShell 7 where it exists, Windows PowerShell where it does not. The
rem harness and its suite are both 5.1-clean and the documentation says so, so
rem hard-requiring pwsh here would have made the self-sabotage sweep the one
rem part of this tool that silently needs PowerShell 7 installed. Raised in the
rem PR #64 review.
set "SABOTAGE_PS=pwsh"
where pwsh >nul 2>&1 || set "SABOTAGE_PS=powershell"

%SABOTAGE_PS% -NoProfile -File "%CD%\tools\test-run-sabotage.ps1"
exit /b %ERRORLEVEL%
