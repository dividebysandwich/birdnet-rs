@echo off
rem Download the PRE-CONVERTED BirdNET model (+ labels, taxonomy, range model)
rem that birdnet-rs publishes as GitHub release assets. Used by the Windows .zip
rem launcher to fetch the model on first run. Requires curl.exe (bundled with
rem Windows 10 1803+). The model is BirdNET GLOBAL 6K v2.4, CC BY-NC-SA 4.0
rem (Cornell Lab) — see MODEL_LICENSE.txt. Non-commercial use + attribution.
rem
rem Usage: fetch-model.bat [OUT_DIR]   (default OUT_DIR: models)
setlocal
set "OUT=%~1"
if "%OUT%"=="" set "OUT=models"
if not defined BIRDNET_MODEL_BASE_URL set "BIRDNET_MODEL_BASE_URL=https://github.com/dividebysandwich/birdnet-rs/releases/latest/download"
if not exist "%OUT%" mkdir "%OUT%"

for %%F in (
  BirdNET_GLOBAL_6K_V2.4.onnx
  BirdNET_GLOBAL_6K_V2.4_RangeModel.onnx
  BirdNET_GLOBAL_6K_V2.4_Labels_en_us.txt
  eBird_taxonomy_codes_2021E.json
  MODEL_LICENSE.txt
) do (
  if exist "%OUT%\%%F" (
    echo exists, skipping: %OUT%\%%F
  ) else (
    echo downloading %%F ...
    curl -fL --retry 3 -o "%OUT%\%%F.part" "%BIRDNET_MODEL_BASE_URL%/%%F" || goto :fail
    move /y "%OUT%\%%F.part" "%OUT%\%%F" >nul
  )
)

echo.
echo model ready in: %OUT%
echo NOTE: the BirdNET model is CC BY-NC-SA 4.0 (Cornell Lab) - see %OUT%\MODEL_LICENSE.txt
exit /b 0

:fail
echo error: model download failed >&2
exit /b 1
