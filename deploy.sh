#!/bin/bash
# WCES 闁?缂佹梻鍋犵粋灞剧▔閳ь剟鏌ㄩ鈧崕瀵哥磾閼艰泛澹栭柡?# 闁革负鍔庨幉娲媰?RZ/G2L 濞戞挸锕ㄧ换宥囨偘? 闁告凹鍨版慨鈺呭箥閳ь剟寮垫径瀣疀闁?# 濞达綀娉曢弫? ssh root@<RZ_IP> && cd /opt/WCES && ./deploy.sh
# RZ_IP 闁?apply-config.ps1/sh 濞?wces.config.toml [deploy] 婵炲牓娼ч幃鎾愁潰?
set -e

# 闁宠法濯寸粭?濞寸姰鍎扮粭鍛存煀瀹ュ洨鏋傞柣?apply-config.ps1/sh 濞?wces.config.toml 闁煎浜滄慨鈺呭触鐏炵虎鍔勯柕?#    闁归潧顑呮慨鈺傜┍椤旇姤鏆柛娆樺灥閸忔﹢宕烽妸銈囩憮濞戞挴鍋撴繛?apply-config 闁哄啯鍎奸～锔炬啺閸℃瑦纾伴柕?
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m'

echo -e "${CYAN}========================================${NC}"
echo -e "${CYAN}   WCES 闁?缂佹梻鍋犵粋宀€鍖栭懡銈囧煚闂侇喓鍔庣拋?            ${NC}"
echo -e "${CYAN}   闁荤喓鍋犻幆?RZ/G2L + ESP32-C5 閼?3          ${NC}"
echo -e "${CYAN}========================================${NC}"
echo ""

# 闁冲厜鍋撻柍鍏夊亾 闂佹澘绉堕悿?闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋?HTTP_PORT=8080
WS_PORT=8765
UDP_PORT=5005
SENSING_BIN="./rust-server/target/aarch64-unknown-linux-gnu/release/sensing-server"
TRIAGE_UI="./docs/triage-ui/triage.html"
UI_DIR="./ui"

# 闁冲厜鍋撻柍鍏夊亾 1. 闁绘粠鍨伴。銊ノ涢埀顒勫蓟?闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾
echo -e "${CYAN}[1/5] 闁绘粠鍨伴。銊ノ涢埀顒勫蓟?..${NC}"

if [ ! -f "$SENSING_BIN" ]; then
    echo -e "${RED}闂佹寧鐟ㄩ? 闁归潧褰炵粭澶愬礆?sensing-server 濞存粌鐭佺换姗€宕氶懜鍨€ù?{NC}"
    echo "  閻犲洤鍢查崢娑氱磽閺嶎剛妲? cargo build --target aarch64-unknown-linux-gnu --release"
    exit 1
fi

if [ ! -d "$UI_DIR" ]; then
    echo -e "${RED}闂佹寧鐟ㄩ? 闁归潧褰炵粭澶愬礆?UI 闁烩晩鍠栫紞?{NC}"
    exit 1
fi

echo -e "  ${GREEN}闁?{NC} sensing-server: $SENSING_BIN"
echo -e "  ${GREEN}闁?{NC} UI 闁烩晩鍠栫紞? $UI_DIR"
echo -e "  ${GREEN}闁?{NC} 闁告帒妫滈惁鏍ㄧ椤忓洢鈧啴鎯? $TRIAGE_UI"

# 闁冲厜鍋撻柍鍏夊亾 2. 缂佹棏鍨拌ぐ娑樜涢埀顒勫蓟?闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾
echo -e "${CYAN}[2/5] 婵☆偀鍋撻柡灞诲劤椤忣剟宕ｉ敐鍛獥闁?..${NC}"

kill_port() {
    local port=$1
    local pid=""
    if command -v lsof > /dev/null 2>&1; then
        pid=$(lsof -ti :$port 2>/dev/null || true)
    elif command -v fuser > /dev/null 2>&1; then
        pid=$(fuser $port/tcp 2>/dev/null || true)
    elif command -v ss > /dev/null 2>&1; then
        pid=$(ss -tlnp "sport = :$port" 2>/dev/null | grep -oP 'pid=\K\d+' | head -1 || true)
    fi
    if [ -n "$pid" ]; then
        echo "  缂佹棏鍨拌ぐ?$port 閻炴凹鍋勫畷浼存偨?(PID: $pid), 闂佹彃锕ラ弬浣圭▔?.."
        kill -9 $pid 2>/dev/null || true
        sleep 1
    fi
}

kill_port $HTTP_PORT
kill_port $WS_PORT
kill_port $UDP_PORT
echo -e "  ${GREEN}闁?{NC} 缂佹棏鍨拌ぐ娑橆啅閺屻儱娅為柡鈧?

# 闁冲厜鍋撻柍鍏夊亾 3. 闁告凹鍨版慨鈺呭嫉瀹ュ懎顫?闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾
# 濠㈣泛绉撮崺妤呭礆閸℃氨妲氬ù鐙€浜ｉ妴鍐儎濡搫鐓?UI 闁烩晩鍠栫紞?echo -e "${CYAN}[3/5] 闂侇喓鍔庣拋鏌ュ礆閸℃氨妲氬ù鐙€浜ｉ妴鍐儎?..${NC}"
cp "$TRIAGE_UI" "$UI_DIR/triage.html"
echo -e "  ${GREEN}闁?{NC} triage.html 闁?$UI_DIR/"

echo -e "${CYAN}[4/5] 闁告凹鍨版慨?Sensing Server...${NC}"

$SENSING_BIN \
    --http-port $HTTP_PORT \
    --ws-port $WS_PORT \
    --udp-port $UDP_PORT \
    --ui-path "$UI_DIR" \
    --bind-addr 0.0.0.0 \
    --source auto \
    --config wces.config.toml \
    --data-dir rust-server \
    > /tmp/wces-server.log 2>&1 &

SERVER_PID=$!
echo -e "  ${GREEN}闁?{NC} Server PID: $SERVER_PID"

# 闁冲厜鍋撻柍鍏夊亾 4. 濡ょ姴鐭侀惁澶愬嫉瀹ュ懎顫?闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾
echo -e "${CYAN}[5/5] 濡ょ姴鐭侀惁澶愬嫉瀹ュ懎顫?..${NC}"
sleep 2

check_http() {
    if curl -s -o /dev/null -w "%{http_code}" http://localhost:$HTTP_PORT/ > /dev/null 2>&1; then
        echo -e "  ${GREEN}闁?{NC} HTTP 闁哄牆绉存慨?(缂佹棏鍨拌ぐ?$HTTP_PORT)"
    else
        echo -e "  ${RED}闁?{NC} HTTP 闁哄牆绉存慨鐔煎籍閻樺弶鎯欓幖? 闁哄被鍎冲﹢鍛村籍閵夈儳绠? tail /tmp/wces-server.log"
    fi
}

check_ws() {
    if command -v ss > /dev/null 2>&1; then
        if ss -tln "sport = :$WS_PORT" 2>/dev/null | grep -q ":$WS_PORT"; then
            echo -e "  ${GREEN}闁?{NC} WebSocket 闁哄牆绉存慨?(缂佹棏鍨拌ぐ?$WS_PORT)"
        else
            echo -e "  ${YELLOW}闁?{NC} WebSocket 缂佹棏鍨拌ぐ娑㈠嫉椤忓嫭鐨戠紓?
        fi
    elif command -v lsof > /dev/null 2>&1; then
        if lsof -i :$WS_PORT > /dev/null 2>&1; then
            echo -e "  ${GREEN}闁?{NC} WebSocket 闁哄牆绉存慨?(缂佹棏鍨拌ぐ?$WS_PORT)"
        else
            echo -e "  ${YELLOW}闁?{NC} WebSocket 缂佹棏鍨拌ぐ娑㈠嫉椤忓嫭鐨戠紓?
        fi
    else
        echo -e "  ${YELLOW}闁?{NC} WebSocket 缂佹棏鍨拌ぐ娑㈡偐閼哥鍋撴担瑙勶骏婵炲娲橀ˉ鍛?(缂傚倸鎼惃?ss/lsof)"
    fi
}

check_udp() {
    if command -v ss > /dev/null 2>&1; then
        if ss -uln "sport = :$UDP_PORT" 2>/dev/null | grep -q ":$UDP_PORT"; then
            echo -e "  ${GREEN}闁?{NC} UDP CSI 闁规亽鍎查弫?(缂佹棏鍨拌ぐ?$UDP_PORT)"
        else
            echo -e "  ${YELLOW}闁?{NC} UDP 缂佹棏鍨拌ぐ娑㈠嫉椤忓嫭鐨戠紓?(缂佹稑顦欢?ESP32 闁煎搫鍊婚崑锝夊矗閹达腹鍋撴担瑙勬闁?"
        fi
    elif command -v lsof > /dev/null 2>&1; then
        if lsof -i :$UDP_PORT > /dev/null 2>&1; then
            echo -e "  ${GREEN}闁?{NC} UDP CSI 闁规亽鍎查弫?(缂佹棏鍨拌ぐ?$UDP_PORT)"
        else
            echo -e "  ${YELLOW}闁?{NC} UDP 缂佹棏鍨拌ぐ娑㈠嫉椤忓嫭鐨戠紓?(缂佹稑顦欢?ESP32 闁煎搫鍊婚崑锝夊矗閹达腹鍋撴担瑙勬闁?"
        fi
    else
        echo -e "  ${YELLOW}闁?{NC} UDP 缂佹棏鍨拌ぐ娑㈡偐閼哥鍋撴担瑙勶骏婵炲娲橀ˉ鍛?(缂傚倸鎼惃?ss/lsof), 缂佹稑顦欢鐔煎极閻楀牆绁?.."
    fi
}

check_http
check_ws
check_udp

# 闁冲厜鍋撻柍鍏夊亾 5. 閻庣懓鏈崹?闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾闁冲厜鍋撻柍鍏夊亾
echo ""
echo -e "${CYAN}[5/5] 闂侇喓鍔庣拋鑼偓鐟版湰閸?${NC}"
echo ""
echo -e "  ${GREEN}闁?{NC} 闁告帒妫滈惁鏍ㄧ椤忓洢鈧啴鎯?  ${CYAN}http://localhost:$HTTP_PORT/ui/triage.html${NC}"
echo -e "  ${GREEN}闁?{NC} 3D 闁告瑯鍨甸～瀣礌?    ${CYAN}http://localhost:$HTTP_PORT${NC}"
echo -e "  ${GREEN}闁?{NC} 闁哄牆绉存慨鐔煎闯閵婏附锛夐煫?   ${CYAN}tail -f /tmp/wces-server.log${NC}"
echo -e "  ${GREEN}闁?{NC} 闁稿绮嶉娑㈠嫉瀹ュ懎顫?     ${CYAN}kill $SERVER_PID${NC}"
echo ""
echo -e "${CYAN}缂佹稑顦欢?ESP32-C5 闁煎搫鍊婚崑锝夊矗閹达腹鍋?CSI 闁轰胶澧楀畵?..${NC}"
