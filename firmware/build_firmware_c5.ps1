# ESP32-C5 CSI Node — Build & Flash Script
# Uses RISC-V toolchain (riscv32-esp-elf) instead of Xtensa (xtensa-esp-elf).
# REQUIRES ESP-IDF v5.5+ (v5.4 has C5 5GHz CSI cache coherency bug).
#
# Before running:
#   1. Verify RISC-V toolchain: dir C:\Espressif\tools\riscv32-esp-elf
#   2. Verify C5 target:     idf.py --list-targets | findstr esp32c5
#   3. Adjust $env:IDF_PATH below to match your installation.
#   4. Run apply-config.ps1 -NodeId <N> first to generate sdkconfig.defaults.
#
# Usage:
#   .\build_firmware_c5.ps1 -FlashPort COM3          # Build & flash
#   .\build_firmware_c5.ps1 -FlashPort COM3 -Clean   # Full rebuild & flash
#   .\build_firmware_c5.ps1 -BuildOnly               # Build only, no flash

param(
    [string]$FlashPort = "",
    [switch]$BuildOnly,
    [switch]$Clean
)

# Remove MSYS environment variables that trigger ESP-IDF's MinGW rejection
Remove-Item env:MSYSTEM -ErrorAction SilentlyContinue
Remove-Item env:MSYSTEM_CARCH -ErrorAction SilentlyContinue
Remove-Item env:MSYSTEM_CHOST -ErrorAction SilentlyContinue
Remove-Item env:MSYSTEM_PREFIX -ErrorAction SilentlyContinue
Remove-Item env:MINGW_CHOST -ErrorAction SilentlyContinue
Remove-Item env:MINGW_PACKAGE_PREFIX -ErrorAction SilentlyContinue
Remove-Item env:MINGW_PREFIX -ErrorAction SilentlyContinue

# ---- CONFIGURATION ----
# ESP-IDF v6.0.1 (ESP32-C5 full stable support)
$env:IDF_PATH = "C:\esp\v6.0.1\esp-idf"
$env:IDF_TOOLS_PATH = "C:\Espressif\tools"
$env:IDF_PYTHON_ENV_PATH = "C:\Espressif\tools\python\v6.0.1\venv"

# RISC-V toolchain for ESP32-C5 (riscv32-esp-elf)
# Adjust the version number to match your installed toolchain.
$env:PATH = "C:\Espressif\tools\riscv32-esp-elf\esp-15.2.0_20251204\riscv32-esp-elf\bin;C:\Espressif\tools\cmake\4.0.3\bin;C:\Espressif\tools\ninja\1.12.1;C:\Espressif\tools\ccache\4.12.1\ccache-4.12.1-windows-x86_64;C:\Espressif\tools\idf-exe\1.0.3;C:\Espressif\tools\python\v6.0.1\venv\Scripts;$env:PATH"

# Firmware directory (relative to script location)
Set-Location $PSScriptRoot

$python = "$env:IDF_PYTHON_ENV_PATH\Scripts\python.exe"
$idf = "$env:IDF_PATH\tools\idf.py"

Write-Host "=== ESP32-C5 CSI Node Build ==="
Write-Host "Target: esp32c5 (RISC-V 32-bit, WiFi 6)"
Write-Host "IDF:    $env:IDF_PATH"
Write-Host ""

if ($Clean) {
    Write-Host "=== Cleaning stale build cache ==="
    & $python $idf fullclean
}

Write-Host "=== Setting target to ESP32-C5 ==="
& $python $idf set-target esp32c5

Write-Host "=== Building firmware ==="
& $python $idf build

if ($LASTEXITCODE -eq 0) {
    Write-Host ""
    Write-Host "=== Build succeeded! ==="
    Write-Host "Binary: build\esp32-csi-node.bin"
    Write-Host ""

    if ($BuildOnly) {
        Write-Host "Build-only mode — skipping flash."
        Write-Host "Flash manually: idf.py -p <COMx> flash"
    } elseif ($FlashPort) {
        Write-Host "=== Flashing to $FlashPort ==="
        & $python $idf -p $FlashPort flash
    } else {
        Write-Host "No flash port specified. To flash:"
        Write-Host "  .\build_firmware_c5.ps1 -FlashPort COMx"
        Write-Host "  or: idf.py -p COMx flash"
    }
} else {
    Write-Host ""
    Write-Host "=== Build failed with exit code $LASTEXITCODE ==="
    Write-Host ""
    Write-Host "Troubleshooting:"
    Write-Host "  1. ESP-IDF v5.5+ required for full C5 CSI support"
    Write-Host "     v5.4 will build but 5GHz CSI is broken (use 2.4GHz only)"
    Write-Host "  2. Check riscv32-esp-elf toolchain:"
    Write-Host "     dir C:\Espressif\tools\riscv32-esp-elf"
    Write-Host "  3. Check esp32c5 is a supported target:"
    Write-Host "     idf.py --list-targets"
    Write-Host "  4. Ensure sdkconfig.defaults exists (run ..\..\apply-config.ps1 -NodeId N)"
    Write-Host "  5. See README.md for full compatibility notes"
}
