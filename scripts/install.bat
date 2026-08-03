@echo off
rem Downloads the latest fakestream release for Windows x86_64, verifies it
rem against the release's SHA256SUMS, and unpacks it into the current
rem directory. Needs Windows 10 or later, which ships curl and tar.
rem
rem Usage:
rem   curl -fsSL -o install.bat https://raw.githubusercontent.com/ektotv/fakestream/main/scripts/install.bat && install.bat

setlocal enabledelayedexpansion
set REPO=ektotv/fakestream

rem The latest tag, read from the API. PowerShell parses the JSON, since
rem batch cannot and it is present on every supported Windows.
for /f "usebackq delims=" %%v in (`powershell -NoProfile -Command "(Invoke-RestMethod 'https://api.github.com/repos/%REPO%/releases/latest').tag_name"`) do set TAG=%%v
if not defined TAG (
  echo could not find the latest release 1>&2
  exit /b 1
)

set NAME=fakestream-%TAG%-windows-x86_64
set BASE=https://github.com/%REPO%/releases/download/%TAG%

echo downloading fakestream %TAG% for windows-x86_64
curl -fsSL -O %BASE%/%NAME%.zip || exit /b 1
curl -fsSL -O %BASE%/SHA256SUMS || exit /b 1

rem Verify before unpacking: hash the download, then require that hash to
rem appear in the published sums file.
set HASH=
for /f %%h in ('certutil -hashfile %NAME%.zip SHA256 ^| findstr /r "^[0-9a-f][0-9a-f]*$"') do set HASH=%%h
if not defined HASH (
  echo could not hash %NAME%.zip 1>&2
  exit /b 1
)
findstr /i /c:"%HASH%" SHA256SUMS >nul || (
  echo checksum mismatch for %NAME%.zip 1>&2
  exit /b 1
)

rem The zip holds the files directly, so they are unpacked into a named
rem directory to match what the tarballs produce.
mkdir %NAME% 2>nul
tar -xf %NAME%.zip -C %NAME% || exit /b 1
del %NAME%.zip SHA256SUMS

echo.
echo unpacked %NAME%\
echo run it:
echo   %NAME%\fakestream.exe
echo.
echo or move fakestream.exe into a folder on your PATH to run it from anywhere
endlocal
