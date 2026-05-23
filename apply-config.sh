#!/bin/bash
# apply-config.sh — 读取 wces.config.toml 并应用到各子系统
# 用法: ./apply-config.sh [--dry-run] [node_id]
#   --dry-run  仅打印将要应用的配置, 不修改文件
#   node_id    指定节点编号 (1/2/3), 用于生成节点专属配置

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CONFIG_FILE="$SCRIPT_DIR/wces.config.toml"
DRY_RUN=false
NODE_ID=""

# 参数解析
while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run) DRY_RUN=true; shift ;;
        --node=*) NODE_ID="${1#*=}"; shift ;;
        [1-3]) NODE_ID="$1"; shift ;;
        *) echo "未知参数: $1"; exit 1 ;;
    esac
done

if [ ! -f "$CONFIG_FILE" ]; then
    echo "错误: 找不到配置文件 $CONFIG_FILE"
    echo "  请先复制并修改 wces.config.toml"
    exit 1
fi

# ── TOML 解析辅助函数 ──────────────────────────
# (简化版, 不支持嵌套表和多行字符串)

toml_get() {
    # $1: section (如 "firmware")
    # $2: key (如 "node_id")
    # 返回: value
    local section="$1" key="$2"
    local in_section=false
    while IFS= read -r line; do
        # 去除注释
        line="${line%%#*}"
        # 去除首尾空白
        line="$(echo "$line" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
        [ -z "$line" ] && continue
        
        if [[ "$line" =~ ^\[(.*)\]$ ]]; then
            local sec="${BASH_REMATCH[1]}"
            sec="${sec//\"/}"
            if [ "$sec" = "$section" ] || [[ "$sec" == "$section."* ]]; then
                in_section=true
            else
                in_section=false
            fi
            continue
        fi
        
        if $in_section; then
            if [[ "$line" =~ ^([a-z_]+)[[:space:]]*=[[:space:]]*(.*) ]]; then
                local k="${BASH_REMATCH[1]}"
                local v="${BASH_REMATCH[2]}"
                v="${v//\"/}"
                v="$(echo "$v" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
                if [ "$k" = "$key" ]; then
                    echo "$v"
                    return
                fi
            fi
        fi
    done < "$CONFIG_FILE"
}

toml_get_bool() {
    local v=$(toml_get "$1" "$2")
    if [ "$v" = "true" ] || [ "$v" = "yes" ] || [ "$v" = "y" ]; then
        echo "y"
    else
        echo ""
    fi
}

# ── 提取配置值 ──────────────────────────────────
NODE_ID=${NODE_ID:-$(toml_get "firmware" "node_id")}

SSID=$(toml_get "firmware" "wifi_ssid")
PASS=$(toml_get "firmware" "wifi_password")
CHANNEL=$(toml_get "firmware" "csi_channel")
TARGET_IP=$(toml_get "firmware" "target_ip")
TARGET_PORT=$(toml_get "firmware" "target_port")
BAND=$(toml_get "firmware" "csi_band")

HTTP_PORT=$(toml_get "server" "http_port")
WS_PORT=$(toml_get "server" "ws_port")
UDP_PORT=$(toml_get "server" "udp_port")
SOURCE=$(toml_get "server" "source")
UI_PATH=$(toml_get "server" "ui_path")
LOG_FILE=$(toml_get "server" "log_file")

echo "========================================="
echo "  WCES 配置应用 (节点 $NODE_ID)"
echo "========================================="
echo ""
echo "配置来源: $CONFIG_FILE"
echo ""

# ── 1. 固件 sdkconfig ──────────────────────────
echo "[1/3] 生成固件 sdkconfig.defaults ..."

SDKCONFIG="$SCRIPT_DIR/firmware/esp32-c5-csi-node/sdkconfig.defaults.competition"
TMPFILE="$SDKCONFIG.tmp"

cat > "$TMPFILE" << SDKEOF
# ESP32-C5 CSI Node — 竞赛现场配置 (由 apply-config.sh 生成)
# ⚠️ ESP-IDF v5.5+ REQUIRED (v5.4 has C5 CSI cache bug on 5GHz, fixed in v5.5)
# 网络拓扑:
#   RZ/G2L 主控:   ${TARGET_IP}    (静态 IP)
#   ESP32-C5 #1:   192.168.1.10   (SSID: ${SSID}, 信道 ${CHANNEL})
#   ESP32-C5 #2:   192.168.1.11
#   ESP32-C5 #3:   192.168.1.12

# Target
CONFIG_IDF_TARGET="esp32c5"

# WiFi
CONFIG_CSI_WIFI_SSID="${SSID}"
SDKEOF

# 密码处理
if [ -n "$PASS" ] && [ "$PASS" != '""' ]; then
    echo "CONFIG_CSI_WIFI_PASSWORD=\"${PASS}\"" >> "$TMPFILE"
else
    echo "# CONFIG_CSI_WIFI_PASSWORD is not set (open network)" >> "$TMPFILE"
fi

cat >> "$TMPFILE" << SDKEOF
CONFIG_CSI_TARGET_IP="${TARGET_IP}"
CONFIG_CSI_TARGET_PORT=${TARGET_PORT}
CONFIG_CSI_WIFI_CHANNEL=${CHANNEL}
CONFIG_CSI_NODE_ID=${NODE_ID}   # ← 节点 ${NODE_ID}

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

# 关闭显示 (竞赛用外部触屏连 RZ/G2L)
# CONFIG_DISPLAY_ENABLE is not set

# 关闭 mock CSI (竞赛用真实 CSI)
# CONFIG_CSI_MOCK_ENABLED is not set
SDKEOF

if $DRY_RUN; then
    echo "  [DRY-RUN] 将写入: $SDKCONFIG"
    cat "$TMPFILE"
    rm "$TMPFILE"
else
    mv "$TMPFILE" "$SDKCONFIG"
    echo "  ✓ $SDKCONFIG"
fi

# ── 2. 部署脚本 ──────────────────────────────────
echo "[2/3] 更新 deploy.sh ..."

DEPLOY_FILE="$SCRIPT_DIR/deploy.sh"
TMP_DEPLOY="$DEPLOY_FILE.tmp"

sed \
    -e "s/^HTTP_PORT=.*/HTTP_PORT=${HTTP_PORT}/" \
    -e "s/^WS_PORT=.*/WS_PORT=${WS_PORT}/" \
    -e "s/^UDP_PORT=.*/UDP_PORT=${UDP_PORT}/" \
    "$DEPLOY_FILE" > "$TMP_DEPLOY"

if $DRY_RUN; then
    echo "  [DRY-RUN] 将更新端口: HTTP=${HTTP_PORT} WS=${WS_PORT} UDP=${UDP_PORT}"
    rm "$TMP_DEPLOY"
else
    mv "$TMP_DEPLOY" "$DEPLOY_FILE"
    chmod +x "$DEPLOY_FILE"
    echo "  ✓ $DEPLOY_FILE (端口已同步)"
fi

# ── 3. 启动命令提示 ──────────────────────────────
echo "[3/3] 服务端启动参数 ..."

START_CMD="cargo run -p wifi-densepose-sensing-server -- \\
    --source ${SOURCE} \\
    --ui-path ${UI_PATH} \\
    --bind-addr 0.0.0.0 \\
    --http-port ${HTTP_PORT}"

echo ""
echo "========================================="
echo "  配置应用完成!"
echo "========================================="
echo ""
echo "▶ 仿真开发 (无硬件):"
echo "  cd rust-server && $START_CMD"
echo ""
echo "▶ 生产部署 (RZ/G2L):"
echo "  ./deploy.sh"
echo ""
echo "▶ 固件编译 (ESP32-C5):"
echo "  cd firmware/esp32-c5-csi-node"
echo '  idf.py set-target esp32c5 && idf.py build'
echo ""
echo "▶ 更换节点后重新应用:"
echo "  ./apply-config.sh 2    # 节点 2"
echo "  ./apply-config.sh 3    # 节点 3"
echo ""

if ! $DRY_RUN; then
    echo "已生成文件:"
    echo "  固件: $SDKCONFIG"
    echo "  部署: $DEPLOY_FILE"
fi
