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

$SSID        = Get-TomlValue "firmware" "wifi_ssid"
$PASS        = Get-TomlValue "firmware" "wifi_password"
$CHANNEL     = Get-TomlValue "firmware" "csi_channel"
$TARGET_IP   = Get-TomlValue "firmware" "target_ip"
$TARGET_PORT = Get-TomlValue "firmware" "target_port"

# ADR-029: TDM
$TDM_NODE_COUNT = Get-TomlValue "firmware.tdm" "node_count"
$TDM_SLOT = ($NodeId - 1)  # 0-based, aligns with node_id

# ADR-039: Edge intelligence
$EDGE_TIER          = Get-TomlValue "firmware.edge" "tier"
$TOP_K              = Get-TomlValue "firmware.edge" "top_k_count"
$PRESENCE_THRESH    = Get-TomlValue "firmware.edge" "presence_thresh"
$FALL_THRESH        = Get-TomlValue "firmware.edge" "fall_thresh"
$VITAL_WINDOW       = Get-TomlValue "firmware.edge" "vital_window"
$VITAL_INTERVAL     = Get-TomlValue "firmware.edge" "vital_interval_ms"
$POWER_DUTY         = Get-TomlValue "firmware.edge" "power_duty"

# ADR-029: Channel hopping
$HOP_ENABLED = Get-TomlValue "firmware.hop" "enabled"
$HOP_DWELL   = Get-TomlValue "firmware.hop" "dwell_ms"

$HTTP_PORT = Get-TomlValue "server" "http_port"
$WS_PORT   = Get-TomlValue "server" "ws_port"
$UDP_PORT  = Get-TomlValue "server" "udp_port"
$SOURCE    = Get-TomlValue "server" "source"
$UI_PATH   = Get-TomlValue "server" "ui_path"

Write-Host "=========================================" -ForegroundColor Cyan
Write-Host "  WCES 配置应用 (节点 $NodeId / 共 $TDM_NODE_COUNT 节点)" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host ""

# ── 1. 固件 sdkconfig.defaults ──────────────────
Write-Host "[1/3] 生成固件 sdkconfig.defaults ..." -ForegroundColor Cyan

$SdkConfig = Join-Path $ScriptDir "firmware\esp32-c5-csi-node\sdkconfig.defaults"
$PassLine = if ($PASS -and $PASS -ne '""') {
    "CONFIG_CSI_WIFI_PASSWORD=`"$PASS`""
} else {
    "# CONFIG_CSI_WIFI_PASSWORD is not set (open network)"
}

# edge_tier, top_k, presence_thresh, fall_thresh, vital_window, vital_interval_ms
$EdgeTierLine = if ($EDGE_TIER) {
    "CONFIG_EDGE_TIER=$EDGE_TIER"
} else {
    "CONFIG_EDGE_TIER=2"
}
$TopKLine = if ($TOP_K) {
    "CONFIG_EDGE_TOP_K=$TOP_K"
} else {
    "# CONFIG_EDGE_TOP_K not set (default 8)"
}
$VitalIntervalLine = if ($VITAL_INTERVAL) {
    "CONFIG_EDGE_VITAL_INTERVAL_MS=$VITAL_INTERVAL"
} else {
    "# CONFIG_EDGE_VITAL_INTERVAL_MS not set (default 1000)"
}
# fall_thresh in Kconfig is raw u16*1000 value; config file has float in rad/s²
$FallThreshRaw = if ($FALL_THRESH) {
    [int]([float]$FALL_THRESH * 1000)
} else {
    15000
}
$FallThreshLine = "CONFIG_EDGE_FALL_THRESH=$FallThreshRaw"
$PowerDutyLine = if ($POWER_DUTY) {
    "CONFIG_EDGE_POWER_DUTY=$POWER_DUTY"
} else {
    "# CONFIG_EDGE_POWER_DUTY not set (default 100)"
}

$SdkContent = @"
# ESP32-C5 CSI Node — 由 apply-config.ps1 生成 (节点 $NodeId)
# ⚠️ ESP-IDF v5.5+ REQUIRED (v5.4 has C5 CSI cache bug on 5GHz)
# 生成时间: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')
# 网络拓扑:
#   RZ/G2L 主控:   ${TARGET_IP}:${TARGET_PORT}
#   ESP32-C5 #${NodeId}: TDM slot ${TDM_SLOT}/${TDM_NODE_COUNT}

# Target
CONFIG_IDF_TARGET="esp32c5"

# WiFi
CONFIG_CSI_WIFI_SSID="${SSID}"
${PassLine}
CONFIG_CSI_TARGET_IP="${TARGET_IP}"
CONFIG_CSI_TARGET_PORT=${TARGET_PORT}
CONFIG_CSI_WIFI_CHANNEL=${CHANNEL}
CONFIG_CSI_NODE_ID=${NodeId}

# 分区表 (8MB + OTA)
CONFIG_PARTITION_TABLE_CUSTOM=y
CONFIG_PARTITION_TABLE_CUSTOM_FILENAME="partitions_display.csv"
CONFIG_ESPTOOLPY_FLASHSIZE_8MB=y
CONFIG_ESPTOOLPY_FLASHSIZE="8MB"

# CSI 启用
CONFIG_ESP_WIFI_CSI_ENABLED=y

# ADR-039: 边缘智能
${EdgeTierLine}
${TopKLine}
${VitalIntervalLine}
${FallThreshLine}
${PowerDutyLine}

# 编译优化
CONFIG_COMPILER_OPTIMIZATION_SIZE=y

# 日志 (开发用 INFO，竞赛用 WARN)
CONFIG_BOOTLOADER_LOG_LEVEL_WARN=y
CONFIG_LOG_DEFAULT_LEVEL_INFO=y

# 网络
CONFIG_LWIP_SO_RCVBUF=y

# 内存 (C5 400KB SRAM)
CONFIG_ESP_MAIN_TASK_STACK_SIZE=7168

# 关闭显示 (竞赛用外部触屏连 RZ/G2L)
# CONFIG_DISPLAY_ENABLE is not set

# 关闭 mock CSI (真实硬件)
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
    Write-Host "  [DRY-RUN] 将更新: HTTP=${HTTP_PORT} WS=${WS_PORT} UDP=${UDP_PORT} RZ_IP=${TARGET_IP}" -ForegroundColor Yellow
} else {
    $deployContent = Get-Content $DeployFile -Raw
    $deployContent = $deployContent -replace '(?m)^RZ_IP=.*$', "RZ_IP=${TARGET_IP}"
    $deployContent = $deployContent -replace '(?m)^HTTP_PORT=.*$', "HTTP_PORT=${HTTP_PORT}"
    $deployContent = $deployContent -replace '(?m)^WS_PORT=.*$', "WS_PORT=${WS_PORT}"
    $deployContent = $deployContent -replace '(?m)^UDP_PORT=.*$', "UDP_PORT=${UDP_PORT}"
    [System.IO.File]::WriteAllText($DeployFile, $deployContent)
    Write-Host "  [OK] $DeployFile (IP/端口已同步)" -ForegroundColor Green
}

# ── 3. NVS provisioning 提示 ─────────────────────
Write-Host "[3/3] NVS 运行时配置" -ForegroundColor Cyan

# Calculate fall_thresh for NVS (u16, value * 1000)
$NvsFallThresh = if ($FALL_THRESH) {
    [int]([float]$FALL_THRESH * 1000)
} else {
    15000
}
$NvsPresenceThresh = if ($PRESENCE_THRESH -and [float]$PRESENCE_THRESH -ne 0.0) {
    [int]([float]$PRESENCE_THRESH * 1000)
} else {
    0
}

Write-Host ""
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host "  配置应用完成! (节点 $NodeId)" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host ""

Write-Host "  NVS 烧录命令 (节点 $NodeId, 首次烧录后运行):" -ForegroundColor Yellow
Write-Host "    cd firmware\esp32-c5-csi-node"
$nvsCmd = "    python provision.py --port <COMx> --node-id $NodeId --ssid `"$SSID`" --target-ip $TARGET_IP --target-port $TARGET_PORT"
if ($TDM_NODE_COUNT) {
    $nvsCmd += " --tdm-slot $TDM_SLOT --tdm-total $TDM_NODE_COUNT"
}
if ($EDGE_TIER) {
    $nvsCmd += " --edge-tier $EDGE_TIER"
}
if ($TOP_K) {
    $nvsCmd += " --subk-count $TOP_K"
}
if ($VITAL_INTERVAL) {
    $nvsCmd += " --vital-int $VITAL_INTERVAL"
}
if ([float]$FALL_THRESH -gt 0) {
    $nvsCmd += " --fall-thresh $NvsFallThresh"
}
if ($PASS) {
    $nvsCmd += " --password `"$PASS`""
}
Write-Host $nvsCmd -ForegroundColor Gray
Write-Host ""

Write-Host "  固件编译 + 烧录 (节点 $NodeId):" -ForegroundColor Green
Write-Host "    cd firmware\esp32-c5-csi-node"
Write-Host "    idf.py set-target esp32c5"
Write-Host "    idf.py build"
Write-Host "    idf.py -p <COMx> flash"
Write-Host ""

Write-Host "  Windows 开发 (模拟模式):" -ForegroundColor Green
Write-Host "    cd rust-server"
Write-Host "    cargo run -p wifi-densepose-sensing-server -- --source $SOURCE --ui-path $UI_PATH --bind-addr 0.0.0.0 --http-port $HTTP_PORT"
Write-Host ""

Write-Host "  更换节点:" -ForegroundColor Green
Write-Host "    .\apply-config.ps1 -NodeId 2"
Write-Host "    .\apply-config.ps1 -NodeId 3"
Write-Host ""
