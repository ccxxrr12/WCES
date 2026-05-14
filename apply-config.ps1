# apply-config.ps1 — 读取 wces.config.toml 并应用到各子系统 (Windows)
# 用法: .\apply-config.ps1 [-DryRun] [-NodeId 1|2|3]
param(
    [switch]$DryRun,
    [ValidateSet(1, 2, 3)]
    [int]$NodeId = 0
)

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ConfigFile = Join-Path $ScriptDir "wces.config.toml"

if (-not (Test-Path $ConfigFile)) {
    Write-Error "找不到配置文件: $ConfigFile"
    exit 1
}

# ── 解析 TOML (简化版) ──────────────────────────
function Get-TomlValue {
    param($Section, $Key)
    $inSection = $false
    $sectionPrefix = "[$Section"
    
    foreach ($line in Get-Content $ConfigFile) {
        $trimmed = $line -replace '\s*#.*$', '' -replace '^\s+', '' -replace '\s+$', ''
        if (-not $trimmed) { continue }
        
        if ($trimmed -match '^\[(.+)\]$') {
            $sec = $Matches[1] -replace '"', ''
            $inSection = ($sec -eq $Section) -or $sec.StartsWith("$Section.")
            continue
        }
        
        if ($inSection -and $trimmed -match '^(\w+)\s*=\s*(.+)$') {
            if ($Matches[1] -eq $Key) {
                return ($Matches[2] -replace '"', '').Trim()
            }
        }
    }
    return ""
}

# ── 提取配置 ──────────────────────────────────
if ($NodeId -eq 0) { $NodeId = if (($v = Get-TomlValue "firmware" "node_id")) { [int]$v } else { 1 } }

$SSID    = Get-TomlValue "firmware" "wifi_ssid"
$PASS    = Get-TomlValue "firmware" "wifi_password"
$CHANNEL = Get-TomlValue "firmware" "csi_channel"
$TARGET_IP   = Get-TomlValue "firmware" "target_ip"
$TARGET_PORT = Get-TomlValue "firmware" "target_port"

$HTTP_PORT = Get-TomlValue "server" "http_port"
$WS_PORT   = Get-TomlValue "server" "ws_port"
$UDP_PORT  = Get-TomlValue "server" "udp_port"
$SOURCE    = Get-TomlValue "server" "source"
$UI_PATH   = Get-TomlValue "server" "ui_path"

Write-Host "=========================================" -ForegroundColor Cyan
Write-Host "  WCES 配置应用 (节点 $NodeId)" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host ""

# ── 1. 固件 sdkconfig ──────────────────────────
Write-Host "[1/3] 生成固件 sdkconfig.defaults ..." -ForegroundColor Cyan

$SdkConfig = Join-Path $ScriptDir "firmware\esp32-c5-csi-node\sdkconfig.defaults.competition"
$PassLine = if ($PASS -and $PASS -ne '""') {
    "CONFIG_CSI_WIFI_PASSWORD=`"$PASS`""
} else {
    "# CONFIG_CSI_WIFI_PASSWORD is not set (open network)"
}

$SdkContent = @"
# ESP32-C5 CSI Node — 竞赛现场配置 (由 apply-config.ps1 生成)
# ⚠️ ESP-IDF v5.5+ REQUIRED (v5.4 has C5 CSI cache bug on 5GHz, fixed in v5.5)
# 网络拓扑:
#   RZ/V2H 主控:   ${TARGET_IP}    (静态 IP)
#   ESP32-C5 #1:   192.168.1.10   (SSID: ${SSID}, 信道 ${CHANNEL})
#   ESP32-C5 #2:   192.168.1.11
#   ESP32-C5 #3:   192.168.1.12

# Target
CONFIG_IDF_TARGET="esp32c5"

# WiFi
CONFIG_CSI_WIFI_SSID="${SSID}"
${PassLine}
CONFIG_CSI_TARGET_IP="${TARGET_IP}"
CONFIG_CSI_TARGET_PORT=${TARGET_PORT}
CONFIG_CSI_WIFI_CHANNEL=${CHANNEL}
CONFIG_CSI_NODE_ID=${NodeId}   # ← 节点 ${NodeId}

# 分区表 (8MB + OTA)
CONFIG_PARTITION_TABLE_CUSTOM=y
CONFIG_PARTITION_TABLE_CUSTOM_FILENAME="partitions_display.csv"
CONFIG_ESPTOOLPY_FLASHSIZE_8MB=y
CONFIG_ESPTOOLPY_FLASHSIZE="8MB"

# CSI 启用
CONFIG_ESP_WIFI_CSI_ENABLED=y

# 编译优化
CONFIG_COMPILER_OPTIMIZATION_SIZE=y

# 日志 (竞赛现场用 WARN 级别减少串口输出)
CONFIG_BOOTLOADER_LOG_LEVEL_WARN=y
CONFIG_LOG_DEFAULT_LEVEL_WARN=y

# 网络
CONFIG_LWIP_SO_RCVBUF=y

# 内存 (C5 400KB SRAM)
CONFIG_ESP_MAIN_TASK_STACK_SIZE=7168

# 关闭显示 (竞赛用外部触屏连 RZ/V2H)
# CONFIG_DISPLAY_ENABLE is not set

# 关闭 mock CSI (竞赛用真实 CSI)
# CONFIG_CSI_MOCK_ENABLED is not set
"@

if ($DryRun) {
    Write-Host "  [DRY-RUN] 将写入: $SdkConfig" -ForegroundColor Yellow
    Write-Host $SdkContent
} else {
    [System.IO.File]::WriteAllText($SdkConfig, $SdkContent)
    Write-Host "  [OK] $SdkConfig" -ForegroundColor Green
}

# ── 2. 部署脚本 ──────────────────────────────────
Write-Host "[2/3] 更新 deploy.sh ..." -ForegroundColor Cyan

$DeployFile = Join-Path $ScriptDir "deploy.sh"

if ($DryRun) {
    Write-Host "  [DRY-RUN] 将更新端口: HTTP=${HTTP_PORT} WS=${WS_PORT} UDP=${UDP_PORT}" -ForegroundColor Yellow
} else {
    $deployContent = Get-Content $DeployFile -Raw
    $deployContent = $deployContent -replace '(?m)^HTTP_PORT=.*$', "HTTP_PORT=${HTTP_PORT}"
    $deployContent = $deployContent -replace '(?m)^WS_PORT=.*$', "WS_PORT=${WS_PORT}"
    $deployContent = $deployContent -replace '(?m)^UDP_PORT=.*$', "UDP_PORT=${UDP_PORT}"
    [System.IO.File]::WriteAllText($DeployFile, $deployContent)
    Write-Host "  [OK] $DeployFile (端口已同步)" -ForegroundColor Green
}

# ── 3. 烧录命令提示 ──────────────────────────────
Write-Host "[3/3] 命令提示" -ForegroundColor Cyan

$flashPorts = "COM3", "COM4", "COM5"
$flashPort = $flashPorts[$NodeId - 1]

Write-Host ""
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host "  配置应用完成! (节点 $NodeId)" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host ""

Write-Host "  Windows 开发:" -ForegroundColor Green
Write-Host "    cd rust-server"
Write-Host "    cargo run -p wifi-densepose-sensing-server -- --source $SOURCE --ui-path $UI_PATH --bind-addr 0.0.0.0 --http-port $HTTP_PORT"
Write-Host ""

Write-Host "  固件编译 + 烧录 (节点 $NodeId):" -ForegroundColor Green
Write-Host "    cd firmware\esp32-c5-csi-node"
Write-Host "    idf.py set-target esp32c5 && idf.py build"
Write-Host "    idf.py -p $flashPort flash"
Write-Host ""

Write-Host "  更换节点:" -ForegroundColor Green
Write-Host "    .\apply-config.ps1 -NodeId 2"
Write-Host "    .\apply-config.ps1 -NodeId 3"
Write-Host ""
