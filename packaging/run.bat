@echo off
rem Portable launcher for the Windows .zip distribution: points the server at the
rem bundled site\ directory and runs from the unpacked folder, so the daemon
rem writes config.yaml, the SQLite DB, clips\ and images\ next to the binary.
setlocal
set "LEPTOS_SITE_ROOT=%~dp0site"
if not defined LEPTOS_SITE_ADDR set "LEPTOS_SITE_ADDR=0.0.0.0:8080"
cd /d "%~dp0"
"%~dp0birdnet-rs.exe" %*
