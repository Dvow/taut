#!/bin/bash
# Step 1 of the Wayland GPU path: can lavapipe export a dmabuf here, and does a GL Weston show it?
set -u
P=$(cd "$(dirname "$0")" && pwd)
B=$RUNNER_TEMP/probe
mkdir -p $B
echo "== kernel $(uname -r); $(. /etc/os-release && echo $PRETTY_NAME)"
ls -l /dev/udmabuf /dev/dri 2>&1
echo "== weston $(weston --version 2>&1); mesa $(dpkg-query -W -f '${Version}' mesa-vulkan-drivers)"
vulkaninfo --summary 2>&1 | grep -E "deviceName|driverInfo|apiVersion" | head -6
rustc -V
rustc -O --edition 2024 $P/export.rs -o $B/export 2>&1 | grep -E "^error" -A7
rustc -O --edition 2024 $P/show.rs -o $B/show 2>&1 | grep -E "^error" -A7
echo "== export probe"
$B/export

desk() {
  # desk <backend> <renderer> <display or -> : a Weston of its own; runs show under it.
  export XDG_RUNTIME_DIR=$(mktemp -d)
  chmod 700 $XDG_RUNTIME_DIR
  unset TAUT_XVFB
  local X=""
  if [ $1 = x11 ]; then
    Xvfb :$3 -screen 0 900x600x24 -nolisten tcp > $XDG_RUNTIME_DIR/xvfb.log 2>&1 &
    X=$!
    for i in $(seq 50); do [ -S /tmp/.X11-unix/X$3 ] && break; sleep 0.1; done
    export TAUT_XVFB=:$3
  fi
  DISPLAY=:${3/-/0} weston --backend=$1 --renderer=$2 --width=900 --height=600 --socket=probe-wl --no-config > $XDG_RUNTIME_DIR/weston.log 2>&1 &
  local K=$!
  for i in $(seq 100); do [ -S $XDG_RUNTIME_DIR/probe-wl ] && break; sleep 0.1; done
  sleep 1
  echo "######## weston $1 $2"
  WAYLAND_DISPLAY=probe-wl timeout 30 $B/show
  kill $K $X 2>/dev/null
  wait $K $X 2>/dev/null
  echo "-- weston said:"; grep -v "^$" $XDG_RUNTIME_DIR/weston.log | grep -iE "dmabuf|renderer|egl|error|fail|GL_RENDERER|format" | head -20
}
desk x11 gl 31
desk headless gl -
desk headless pixman -
