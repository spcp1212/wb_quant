@echo off
rem One-click build of wb_quant.exe on Windows.
rem Installs Rust (GNU toolchain, no Visual Studio needed) if it is missing.
cd /d "%~dp0"
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
where cargo >NUL 2>NUL
if errorlevel 1 (
  echo [1/2] Rust not found - installing rustup ^(about 5 min^)...
  curl.exe -L -o "%TEMP%\rustup-init.exe" https://win.rustup.rs/x86_64
  "%TEMP%\rustup-init.exe" -y --default-host x86_64-pc-windows-gnu --profile minimal
)
where cargo >NUL 2>NUL
if errorlevel 1 (
  echo Rust installation failed. Install it from https://rustup.rs and run this file again.
  pause
  exit /b 1
)
echo [2/2] Building wb_quant.exe ^(first build takes a few minutes^)...
cargo build --release
if errorlevel 1 (
  echo Build failed - see messages above.
  pause
  exit /b 1
)
copy /y target\release\wb_quant.exe . >nul
echo.
echo Done: %CD%\wb_quant.exe
pause
