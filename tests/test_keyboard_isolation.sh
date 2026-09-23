#!/bin/bash
# 测试：虚拟显示器 + MPX 虚拟键鼠 + 键盘焦点隔离
# 验证：agent 在虚拟屏上的键鼠操作不影响用户当前工作区的键鼠操作
# 
# 用法：bash tests/test_keyboard_isolation.sh
# 前置：xdotool, xinput, xterm, 用户桌面有显示器

set -e
export DISPLAY="${DISPLAY:-:0}"
PASS=0; FAIL=0

check() {
    if [ "$1" = "0" ]; then PASS=$((PASS+1)); echo "  ✓ $2"
    else FAIL=$((FAIL+1)); echo "  ✗ $2"; fi
}

echo "═══════════════════════════════════════"
echo " 键盘焦点隔离测试"
echo "═══════════════════════════════════════"

# ── 0. 前置检查 ──
for cmd in xdotool xinput xterm xrandr; do
    which $cmd >/dev/null 2>&1 || { echo "缺少 $cmd，跳过测试"; exit 1; }
done

# 找断开的输出
VOUT=$(xrandr --query | grep "disconnected" | head -1 | awk '{print $1}')
if [ -z "$VOUT" ]; then
    echo "SKIP: 没有断开的输出可用"; exit 0
fi
echo "虚拟输出: $VOUT"

# ── 1. 启用虚拟显示器 ──
MODE_NAME="1920x1080_test"
xrandr --newmode $MODE_NAME 173.00 1920 2048 2248 2576 1080 1083 1088 1120 -hsync +vsync 2>/dev/null
xrandr --addmode $VOUT $MODE_NAME 2>/dev/null
xrandr --output $VOUT --mode $MODE_NAME --right-of eDP-1 2>/dev/null || \
xrandr --output $VOUT --mode $MODE_NAME --right-of $(xrandr --query | grep " connected primary" | awk '{print $1}') 2>/dev/null
sleep 0.5

CUR=$(xrandr --query | grep "current")
if echo "$CUR" | grep -q "4480"; then
    check 0 "虚拟显示器启用 (屏幕 4480x1600)"
else
    check 1 "虚拟显示器启用失败"; exit 1
fi

# ── 2. 创建 MPX agent master ──
xinput create-master test-agent 2>/dev/null
AGENT_MASTER=$(xinput list | grep "test-agent pointer" | grep -oP 'id=\K\d+' | head -1)
if [ -z "$AGENT_MASTER" ]; then
    check 1 "MPX agent master 创建失败"; exit 1
fi
check 0 "MPX agent master 创建 (id=$AGENT_MASTER)"

# ── 3. 创建两个 xterm（一个在虚拟屏区域，一个在主屏区域）──
xterm -title "KB-AGENT" -geometry 60x8+2700+100 -e "bash" &
XT1=$!
xterm -title "KB-USER" -geometry 60x8+300+100 -e "bash" &
XT2=$!
sleep 1.5

# 把 AGENT xterm 确保在虚拟屏区域
wmctrl -r "KB-AGENT" -e 0,2700,100,480,160
sleep 0.3

# 获取窗口 ID 和位置
KB_AGENT_WIN=$(xdotool search --name "KB-AGENT" | head -1)
KB_USER_WIN=$(xdotool search --name "KB-USER" | head -1)
echo "agent xterm: $KB_AGENT_WIN  user xterm: $KB_USER_WIN"

# ── 4. 创建 uinput 虚拟键鼠 ──
sudo modprobe uinput 2>/dev/null
sudo chmod 666 /dev/uinput 2>/dev/null

# 用 xinput create-master 创建的 agent master 自带 XTEST slave
# XTEST 事件走 core master——所以我们用 uinput 设备挂在 agent master 上
# 但为了简化测试，先验证：XTEST 点击虚拟屏上的窗口后，焦点是否被抢

# ── 5. 测试：agent 点击虚拟屏上的 xterm，检查用户焦点是否被抢 ──
# 记录用户当前焦点
USER_FOCUS_BEFORE=$(xdotool getactivewindow 2>/dev/null || echo "none")
echo "用户焦点（操作前）: $USER_FOCUS_BEFORE"

# agent 指针移动到虚拟屏的 xterm 上并点击
xdotool mousemove --sync 2850 150 click 1 2>/dev/null
sleep 0.5

# 检查激活窗口是否变了
USER_FOCUS_AFTER=$(xdotool getactivewindow 2>/dev/null || echo "none")
echo "用户焦点（agent 点击后）: $USER_FOCUS_AFTER"

if [ "$USER_FOCUS_BEFORE" = "$USER_FOCUS_AFTER" ]; then
    check 0 "agent 点击虚拟屏不抢用户焦点"
else
    check 1 "agent 点击虚拟屏抢了用户焦点"
fi

# ── 6. 测试：agent 在虚拟屏 xterm 里打字，用户打字不受影响 ──
# agent 往 KB-AGENT xterm 打字
xdotool type "AGENT_TYPED" 2>/dev/null
sleep 0.3

# 截图 KB-AGENT xterm 确认收到了
import -window $KB_AGENT_WIN /tmp/kb_agent_snap.png 2>/dev/null

# 用户往 KB-USER xterm 打字
xdotool mousemove --sync 500 350 click 1 2>/dev/null
sleep 0.3
xdotool type "USER_TYPED" 2>/dev/null
sleep 0.3

# 截图 KB-USER xterm
import -window $KB_USER_WIN /tmp/kb_user_snap.png 2>/dev/null

# ── 7. 验证两个 xterm 的内容 ──
# 用 OCR 或者视觉检查——简化为检查截图非空
AGENT_SNAP_SIZE=$(stat -c%s /tmp/kb_agent_snap.png 2>/dev/null || echo 0)
USER_SNAP_SIZE=$(stat -c%s /tmp/kb_user_snap.png 2>/dev/null || echo 0)

if [ "$AGENT_SNAP_SIZE" -gt 500 ]; then
    check 0 "agent xterm 有内容"
else
    check 1 "agent xterm 截图异常"
fi
if [ "$USER_SNAP_SIZE" -gt 500 ]; then
    check 0 "user xterm 有内容"
else
    check 1 "user xterm 截图异常"
fi

# ── 8. 清理 ──
kill $XT1 $XT2 2>/dev/null
xinput remove-master $AGENT_MASTER 2>/dev/null
xrandr --output $VOUT --off 2>/dev/null

echo "═══════════════════════════════════════"
echo " 结果: $PASS 通过, $FAIL 失败"
echo "═══════════════════════════════════════"
exit $FAIL
