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
rem would then be indistinguishable from a real finding. That includes a valid
rem verb with anything extra after it, or the wrong number of operands. It exits
rem with the process-start code the harness reports as INFRASTRUCTURE instead,
rem which is loud and never a catch.
rem
rem The directive is validated as a whole line, against every valid form, BEFORE
rem anything is read out of it: a line is only ever split into words once it is
rem known to hold nothing but one of those forms, so no character in a mistyped
rem one -- a quote, an ampersand -- reaches the command interpreter as syntax.
rem
rem Only *.faux files are searched, so a directive quoted in documentation or in
rem a manifest cannot be mistaken for a plan.

rem Every *.faux file, read through `more`, into a temporary file. `more` ends
rem every line with CR LF whatever the file used, and findstr's `$` and `/X` only
rem match at a CR: against a file written with bare LFs -- which is what most of
rem this repository's files are -- a valid directive would be read as invalid.
rem
rem The file goes in a directory this run made for itself. Stubs run in parallel,
rem and %RANDOM% alone is not enough to tell them apart: it is seeded from the
rem clock, so processes started together draw the same numbers, and two stubs
rem sharing a file would read each other's plans. mkdir either creates the
rem directory or fails because it exists, atomically, so a name is only ever
rem used by the process that made it.
set "FAUX_TRIES=0"
:make_work
set /a FAUX_TRIES+=1
if %FAUX_TRIES% GTR 100 (
  echo faux-cargo: could not make a working directory under %TEMP% 1>&2
  goto :refuse
)
set "FAUX_WORK=%TEMP%\faux-cargo-%RANDOM%%RANDOM%"
mkdir "%FAUX_WORK%" 2>nul || goto :make_work
set "FAUX_LINES=%FAUX_WORK%\lines.txt"
(for /r %%f in (*.faux) do @more "%%f") > "%FAUX_LINES%" 2>nul

rem How many directive lines there are, and how many of them are one of the
rem valid forms. findstr's regular expressions have no alternation, so each form
rem is its own /C: pattern; ` *$` allows trailing spaces and nothing else.
set "FAUX_COUNT=0"
for /f %%n in ('findstr /B /C:"// faux:" "%FAUX_LINES%" ^| find /c /v ""') do set "FAUX_COUNT=%%n"

rem No directive: the baseline, or a patch that is not a faux one. It passes.
if %FAUX_COUNT%==0 (
  rmdir /s /q "%FAUX_WORK%" 2>nul
  exit /b 0
)

set "FAUX_VALID=0"
for /f %%n in ('findstr /B /R /C:"// faux: pass *$" /C:"// faux: fail *$" /C:"// faux: hang *$" /C:"// faux: build-fail *$" /C:"// faux: sleep [0-9][0-9]* pass *$" /C:"// faux: sleep [0-9][0-9]* fail *$" "%FAUX_LINES%" ^| find /c /v ""') do set "FAUX_VALID=%%n"

rem Exactly one directive, and it is valid. Anything else is refused in every
rem phase, so a typo is loud and not only in the phase it would have governed.
if not %FAUX_COUNT%==1 goto :bad
if not %FAUX_VALID%==1 goto :bad

set "FAUX_VERB="
set "FAUX_A="
set "FAUX_B="
for /f "tokens=3,4,5" %%a in ('findstr /B /C:"// faux:" "%FAUX_LINES%"') do (
  set "FAUX_VERB=%%a"
  set "FAUX_A=%%b"
  set "FAUX_B=%%c"
)
rmdir /s /q "%FAUX_WORK%" 2>nul

rem The seconds are decimal. `set /a` reads a leading zero as octal, so `08` would
rem be an error and `010` would wait eight seconds, not ten; the validated digits
rem have their leading zeros stripped first, and nothing left means zero.
if not "%FAUX_VERB%"=="sleep" goto :valid
set "FAUX_N="
for /f "tokens=* delims=0" %%z in ("%FAUX_A%") do set "FAUX_N=%%z"
if not defined FAUX_N set "FAUX_N=0"
set "FAUX_A=%FAUX_N%"

rem A sleep long enough to overflow set /a would ping for the wrong time; a bound
rem of five digits is a day and more. Judged on the value, after the zeros.
echo %FAUX_A%| findstr /R "^[0-9][0-9][0-9][0-9][0-9][0-9]" >nul && goto :bad_quiet

:valid
rem The build phase: the harness asks for --no-run first. Only build-fail touches
rem it; every other directive describes the run.
echo %* | findstr /C:"--no-run" >nul && goto :build

if "%FAUX_VERB%"=="pass" exit /b 0
if "%FAUX_VERB%"=="build-fail" exit /b 0
if "%FAUX_VERB%"=="fail" goto :test_failed
if "%FAUX_VERB%"=="hang" goto :hang

rem sleep <n> pass|fail. `ping -n k` waits about k-1 seconds. The line says how
rem long, which is what lets a test see the value that was read without waiting
rem it out.
echo faux-cargo: sleeping %FAUX_A% second(s)
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
echo faux-cargo: expected exactly one valid "// faux: ..." directive in *.faux; found %FAUX_COUNT%, of which %FAUX_VALID% valid. The lines read: 1>&2
findstr /B /C:"// faux:" "%FAUX_LINES%" 1>&2
rmdir /s /q "%FAUX_WORK%" 2>nul
goto :refuse

:bad_quiet
echo faux-cargo: expected exactly one valid "// faux: ..." directive; a sleep of six or more digits is not valid: %FAUX_A% 1>&2

:refuse
rem STATUS_DLL_INIT_FAILED, the code the harness reports as INFRASTRUCTURE (see
rem Get-ProcessStartFailure in common.ps1) rather than as a result.
exit /b -1073741502
