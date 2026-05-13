#!/bin/bash
# WCES — 竞赛一键部署脚本
# 在瑞萨 RZ/V2H 上运行, 启动所有服务
# 使用: ssh root@192.168.1.1 && cd /opt/WCES && ./deploy.sh

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
NC='\033[0m'

echo -e "${CYAN}========================================${NC}"
echo -e "${CYAN}   WCES — 竞赛系统部署             ${NC}"
echo -e "${CYAN}   瑞萨 RZ/V2H + ESP32-C5 × 3          ${NC}"
echo -e "${CYAN}========================================${NC}"
echo ""

# ── 配置 ─────────────────────────────────────────
HTTP_PORT=8080
WS_PORT=8765
UDP_PORT=5005
SENSING_BIN="./rust-server/target/aarch64-unknown-linux-gnu/release/sensing-server"
TRIAGE_UI="./docs/triage-ui/triage.html"
UI_DIR="./ui"

# ── 1. 环境检查 ──────────────────────────────────
echo -e "${CYAN}[1/5] 环境检查...${NC}"

if [ ! -f "$SENSING_BIN" ]; then
    echo -e "${RED}错误: 找不到 sensing-server 二进制文件${NC}"
    echo "  请先编译: cargo build --target aarch64-unknown-linux-gnu --release"
    exit 1
fi

if [ ! -d "$UI_DIR" ]; then
    echo -e "${RED}错误: 找不到 UI 目录${NC}"
    exit 1
fi

echo -e "  ${GREEN}✓${NC} sensing-server: $SENSING_BIN"
echo -e "  ${GREEN}✓${NC} UI 目录: $UI_DIR"
echo -e "  ${GREEN}✓${NC} 分诊仪表盘: $TRIAGE_UI"

# ── 2. 端口检查 ──────────────────────────────────
echo -e "${CYAN}[2/5] 检查端口占用...${NC}"

kill_port() {
    local port=$1
    local pid=$(lsof -ti :$port 2>/dev/null || true)
    if [ -n "$pid" ]; then
        echo "  端口 $port 被占用 (PID: $pid), 释放中..."
        kill -9 $pid 2>/dev/null || true
        sleep 1
    fi
}

kill_port $HTTP_PORT
kill_port $WS_PORT
kill_port $UDP_PORT
echo -e "  ${GREEN}✓${NC} 端口已释放"

# ── 3. 启动服务 ──────────────────────────────────
# 复制分诊仪表盘到 UI 目录
echo -e "${CYAN}[3/5] 部署分诊仪表盘...${NC}"
cp "$TRIAGE_UI" "$UI_DIR/triage.html"
echo -e "  ${GREEN}✓${NC} triage.html → $UI_DIR/"

echo -e "${CYAN}[4/5] 启动 Sensing Server...${NC}"

$SENSING_BIN \
    --http-port $HTTP_PORT \
    --ws-port $WS_PORT \
    --udp-port $UDP_PORT \
    --ui-path "$UI_DIR" \
    --bind-addr 0.0.0.0 \
    --source auto \
    > /tmp/wces-server.log 2>&1 &

SERVER_PID=$!
echo -e "  ${GREEN}✓${NC} Server PID: $SERVER_PID"

# ── 4. 验证服务 ──────────────────────────────────
echo -e "${CYAN}[4/5] 验证服务...${NC}"
sleep 2

check_http() {
    if curl -s -o /dev/null -w "%{http_code}" http://localhost:$HTTP_PORT/ > /dev/null 2>&1; then
        echo -e "  ${GREEN}✓${NC} HTTP 服务 (端口 $HTTP_PORT)"
    else
        echo -e "  ${RED}✗${NC} HTTP 服务无响应, 查看日志: tail /tmp/wces-server.log"
    fi
}

check_ws() {
    if lsof -i :$WS_PORT > /dev/null 2>&1; then
        echo -e "  ${GREEN}✓${NC} WebSocket 服务 (端口 $WS_PORT)"
    else
        echo -e "  ${YELLOW}⚠${NC} WebSocket 端口未就绪"
    fi
}

check_udp() {
    if lsof -i :$UDP_PORT > /dev/null 2>&1; then
        echo -e "  ${GREEN}✓${NC} UDP CSI 接收 (端口 $UDP_PORT)"
    else
        echo -e "  ${YELLOW}⚠${NC} UDP 端口未就绪 (等待 ESP32 节点发送数据)"
    fi
}

check_http
check_ws
check_udp

# ── 5. 完成 ──────────────────────────────────────
echo ""
echo -e "${CYAN}[5/5] 部署完成!${NC}"
echo ""
echo -e "  ${GREEN}▶${NC} 分诊仪表盘:  ${CYAN}http://localhost:$HTTP_PORT/ui/triage.html${NC}"
echo -e "  ${GREEN}▶${NC} 3D 可视化:    ${CYAN}http://localhost:$HTTP_PORT${NC}"
echo -e "  ${GREEN}▶${NC} 服务器日志:   ${CYAN}tail -f /tmp/wces-server.log${NC}"
echo -e "  ${GREEN}▶${NC} 停止服务:     ${CYAN}kill $SERVER_PID${NC}"
echo ""
echo -e "${CYAN}等待 ESP32-C5 节点发送 CSI 数据...${NC}"
