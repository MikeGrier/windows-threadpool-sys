@echo off
rem Copyright (c) Mike Grier. All rights reserved.
rem
rem The "cargo" for sabotage-tiers.json: makes test-check-test-tiers.ps1 look like
rem a cargo test run, so the sweep harness can judge the script that checks the
rem test tiers. The same arrangement as sabotage-records-runner.cmd, which is the
rem one to read for why it works.
rem
rem   --no-run      The build phase. There is nothing to compile, so it succeeds.
rem
rem   --target-dir  Meaningless here, and ignored.
rem
rem The harness runs this with the working directory set to its COPY of the tree,
rem so the suite invoked below is the copy's, and tests the copy's script -- the
rem one carrying the injected defect.

echo %* | findstr /C:"--no-run" >nul && exit /b 0

rem PowerShell 7 where it exists, Windows PowerShell where it does not.
set "SABOTAGE_PS=pwsh"
where pwsh >nul 2>&1 || set "SABOTAGE_PS=powershell"

%SABOTAGE_PS% -NoProfile -File "%CD%\tools\test-check-test-tiers.ps1"
exit /b %ERRORLEVEL%
