#!/bin/bash
# Runs a command under a Weston of its own, on the runner:
#   desk.sh <weston-x11-gl|weston-gl|weston|weston-x11> <display> <command...>
# x11: Weston in a window on an Xvfb of its own (display <display>), whose screen the tests read
# and drive (TAUT_XVFB); else headless. gl: GL renderer (takes dmabufs); else pixman (doesn't).
# The command gets WAYLAND_DISPLAY and TAUT_DESK; a "not tested" in its output is a failure.
desk=$1 n=$2; shift 2
unset WAYLAND_DISPLAY DISPLAY WAYLAND_SOCKET
export XDG_RUNTIME_DIR=$(mktemp -d) GSETTINGS_BACKEND=memory TAUT_SCREEN_SCALE=1
chmod 700 $XDG_RUNTIME_DIR
case $desk in
  *-gl) renderer=gl ;;
  *) renderer=pixman ;;
esac
X=""
if [[ $desk == weston-x11* ]]; then
  Xvfb :$n -screen 0 900x600x24 -nolisten tcp > $XDG_RUNTIME_DIR/xvfb.log 2>&1 &
  X=$!
  for i in $(seq 50); do [ -S /tmp/.X11-unix/X$n ] && break; sleep 0.1; done
  export TAUT_XVFB=:$n TAUT_DESK=weston-x11
  DISPLAY=:$n weston --backend=x11 --renderer=$renderer --width=900 --height=600 --socket=taut-wl --debug --no-config > $XDG_RUNTIME_DIR/desk.log 2>&1 &
else
  export TAUT_DESK=weston
  weston --backend=headless --renderer=$renderer --width=900 --height=600 --socket=taut-wl --debug --no-config > $XDG_RUNTIME_DIR/desk.log 2>&1 &
fi
K=$!
for i in $(seq 100); do [ -S $XDG_RUNTIME_DIR/taut-wl ] && break; sleep 0.1; done
# (Weston's desktop fades in from black for a moment.)
sleep 2
echo "--- $desk ($renderer)"
WAYLAND_DISPLAY=taut-wl "$@" 2>&1 | tee $XDG_RUNTIME_DIR/out.txt
R=${PIPESTATUS[0]}
# Weston first, and only once it has ended its X server: a Weston in an X window whose server
# goes first ends in a fatal X error.
kill $K 2>/dev/null
wait $K 2>/dev/null
if [ -n "$X" ]; then
  kill $X 2>/dev/null
  wait $X 2>/dev/null
fi
echo "--- weston said:"
grep -iE "dmabuf|renderer|error|fail" $XDG_RUNTIME_DIR/desk.log | head -20
if grep -q "not tested" $XDG_RUNTIME_DIR/out.txt; then
  echo "::error::a test was not tested under $desk"
  R=1
fi
rm -rf $XDG_RUNTIME_DIR
exit $R
