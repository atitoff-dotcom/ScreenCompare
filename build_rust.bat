@echo off
setlocal
echo ========================================================
echo   ScreenCompare GUI Build (Rust + eframe/egui)
echo ========================================================
echo.

where cargo >nul 2>nul
if errorlevel 1 goto :no_cargo

echo [1/2] Building release binary (cargo build --release)...
echo Please wait, compiling...
echo.
cargo build --release
if errorlevel 1 goto :build_fail

echo.
echo [2/2] Copying executable...
copy /y target\release\screen_compare.exe ScreenCompare.exe >nul
if errorlevel 1 goto :copy_fail

echo.
echo ========================================================
echo   [SUCCESS] ScreenCompare.exe created successfully!
echo   You can now launch ScreenCompare.exe.
echo ========================================================
goto :end

:no_cargo
echo [ERROR] Rust / Cargo is not found in PATH!
echo Please install Rust from: https://rustup.rs/
goto :end

:build_fail
echo.
echo [ERROR] Compilation failed. See compiler output above.
goto :end

:copy_fail
echo [WARNING] Could not copy binary to root folder.
echo Check target\release\screen_compare.exe directly.
goto :end

:end
pause
