@echo off
rem Copyright (c) Mike Grier.
rem
rem A faux "cargo" whose outcome is PLANNED, so the sabotage harness can be
rem exercised with a predictable mix of passing runs, failing runs and runs that
rem overrun their bound -- with no crate to build and nothing random.
rem
rem Passed to run-sabotage.ps1 as -CargoCommand. See "Faux runs" in
rem tools/README-sabotage.md for the technique; this file is its implementation.
rem
rem THE PLAN LIVES IN THE PATCH. A sabotage's `replace` text puts one directive
rem line in a *.faux file, and this script reads the directive back out of the
rem harness's working copy -- the only place a sabotaged run differs from the
rem baseline. So the plan is declarative, sits beside the entry it governs, needs
rem no state shared between runs (it is correct under sharding and in parallel),
rem and the baseline, which has no directive, passes without being told to.
rem
rem   // faux: pass                 the run passes             (sabotage SURVIVES)
rem   // faux: fail                 the run fails, exit 101    (sabotage CAUGHT)
rem   // faux: hang                 the run never ends         (CAUGHT as HUNG, once
rem                                 the harness's bound kills it)
rem   // faux: sleep <n> pass|fail  the run takes <n> seconds, then passes or fails:
rem                                 how a run is placed just under, or just over,
rem                                 a bound
rem   // faux: build-fail           the BUILD phase fails, exit 101 (the manifest
rem                                 "does not compile")
rem
rem A directive this script does not understand, or two of them, is NOT read as a
rem pass or a failure: either would be scored as a result, and a typo in a plan
rem would then be indistinguishable from a real finding. It exits with the
rem process-start code the harness reports as INFRASTRUCTURE instead, which is
rem loud and never a catch.
rem
rem Only *.faux files are searched, so a directive quoted in documentation or in
rem a manifest cannot be mistaken for a plan.

set "FAUX_COUNT=0"
set "FAUX_VERB="
set "FAUX_A="
set "FAUX_B="
rem Tokens 1 and 2 are the file and "// faux:"; reading from token 2 on means a
rem directive with no verb at all still counts as one, and is refused below
rem rather than read as no directive.
for /f "tokens=2,3,4,5" %%a in ('findstr /S /B /C:"// faux:" *.faux 2^>nul') do (
  set /a FAUX_COUNT+=1
  set "FAUX_VERB=%%b"
  set "FAUX_A=%%c"
  set "FAUX_B=%%d"
)

rem No directive: the baseline, or a patch that is not a faux one. It passes.
if %FAUX_COUNT%==0 exit /b 0
if %FAUX_COUNT% GTR 1 goto :bad

rem Validate before acting, so a typo is loud in every phase and not only the one
rem it would have governed.
if "%FAUX_VERB%"=="pass" goto :valid
if "%FAUX_VERB%"=="fail" goto :valid
if "%FAUX_VERB%"=="hang" goto :valid
if "%FAUX_VERB%"=="build-fail" goto :valid
if "%FAUX_VERB%"=="sleep" (
  echo %FAUX_A%| findstr /R /X "[0-9][0-9]*" >nul || goto :bad
  if "%FAUX_B%"=="pass" goto :valid
  if "%FAUX_B%"=="fail" goto :valid
)
goto :bad

:valid
rem The build phase: the harness asks for --no-run first. Only build-fail touches
rem it; every other directive describes the run.
echo %* | findstr /C:"--no-run" >nul && goto :build

if "%FAUX_VERB%"=="pass" exit /b 0
if "%FAUX_VERB%"=="build-fail" exit /b 0
if "%FAUX_VERB%"=="fail" goto :test_failed
if "%FAUX_VERB%"=="hang" goto :hang

rem sleep <n> pass|fail. `ping -n k` waits about k-1 seconds.
set /a FAUX_PINGS=%FAUX_A%+1
ping -n %FAUX_PINGS% 127.0.0.1 >nul
if "%FAUX_B%"=="pass" exit /b 0
goto :test_failed

:build
if "%FAUX_VERB%"=="build-fail" goto :build_failed
exit /b 0

:build_failed
echo error: faux build failure 1>&2
exit /b 101

:test_failed
echo test failed
exit /b 101

:hang
ping -n 900 127.0.0.1 >nul
exit /b 0

:bad
echo faux-cargo: expected exactly one valid "// faux: ..." directive in *.faux, found %FAUX_COUNT%; last read [%FAUX_VERB% %FAUX_A% %FAUX_B%] 1>&2
rem STATUS_DLL_INIT_FAILED, the code the harness reports as INFRASTRUCTURE (see
rem Get-ProcessStartFailure in common.ps1) rather than as a result.
exit /b -1073741502
