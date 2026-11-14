@echo off
rem Copyright (c) Mike Grier. All rights reserved.
rem
rem The "cargo" for sabotage-io-test-tail.json: makes test-report-io-test-tail.ps1 look
rem like a cargo test run, so the sweep harness can judge report-io-test-tail.ps1. The
rem same arrangement as sabotage2-runner.cmd, which is the one to read for why it works.
rem
rem   --no-run      The build phase. There is nothing to compile, so it succeeds.
rem
rem   --target-dir  Meaningless here, and ignored.
rem
rem The harness runs this with the working directory set to its COPY of the tree,
rem so the suite invoked below is the copy's, and tests the copy's script -- the
rem one carrying the injected defect. The script needs PowerShell 7.

echo %* | findstr /C:"--no-run" >nul && exit /b 0

pwsh -NoProfile -File "%CD%\tools\test-report-io-test-tail.ps1"
exit /b %ERRORLEVEL%
