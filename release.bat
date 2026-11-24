@echo off
setlocal
if "%~1"=="" (
    echo Usage: release.bat 0.1.0
    echo Example: release.bat 0.1.1
    exit /b 1
)
set VER=%~1
powershell -NoProfile -Command "$q = [char]34; (Get-Content Cargo.toml) -replace '^version = .*', ('version = ' + $q + '%VER%' + $q) | Set-Content Cargo.toml"
cargo check
if errorlevel 1 exit /b 1
git add Cargo.toml Cargo.lock src/ .github/ build.rs build_rust.bat assets/ .gitignore release.bat
git commit -m "Release v%VER%"
git tag -a "v%VER%" -m "Release v%VER%"
git push
git push --tags
echo.
echo [DONE] Tag v%VER% pushed! GitHub Actions is building Windows and Linux binaries now.
