#!/usr/bin/env bash
# Термо-сторож на ВМ: стежить за GPU. При перегріві створює СТОП-ФАЙЛ, який зупиняє
# нічний тестовий цикл, і шле Telegram-алерт. Запускається з cron кожні 5 хвилин.
set -uo pipefail

STOP=/opt/wallet-tracker/dataset/THERMAL-STOP
LOG=/opt/wallet-tracker/deploy/thermal.log
ENVF=/opt/wallet-tracker/.env

GPU_WARN=75
GPU_STOP=83
MEM_MIN_MB=400          # менше — ризик OOM

ts() { date '+%F %T'; }

tg() {   # тихо: відсутність токена не має ламати сторожа
  local msg="$1"
  local tok chat
  tok=$(grep -E '^TELEGRAM_BOT_TOKEN=' "$ENVF" 2>/dev/null | cut -d= -f2- | tr -d '"\r')
  chat=$(grep -E '^TELEGRAM_CHAT_ID=' "$ENVF" 2>/dev/null | cut -d= -f2- | tr -d '"\r')
  [ -n "$tok" ] && [ -n "$chat" ] || return 0
  curl -s -m 15 -o /dev/null \
    --data-urlencode "text=$msg" \
    "https://api.telegram.org/bot${tok}/sendMessage?chat_id=${chat}" || true
}

GPU_T=$(nvidia-smi --query-gpu=temperature.gpu --format=csv,noheader,nounits 2>/dev/null | head -1)
GPU_P=$(nvidia-smi --query-gpu=power.draw --format=csv,noheader,nounits 2>/dev/null | head -1)
GPU_U=$(nvidia-smi --query-gpu=utilization.gpu --format=csv,noheader,nounits 2>/dev/null | head -1)
MEM_AV=$(awk '/MemAvailable/{print int($2/1024)}' /proc/meminfo)
LOAD=$(cut -d' ' -f1 /proc/loadavg)

GPU_T=${GPU_T:-0}; GPU_P=${GPU_P:-0}; GPU_U=${GPU_U:-0}

echo "$(ts) GPU=${GPU_T}C ${GPU_P}W util=${GPU_U}% RAM_avail=${MEM_AV}MB load=${LOAD}" >> "$LOG"

# --- перегрів GPU ---
if [ "${GPU_T%.*}" -ge "$GPU_STOP" ] 2>/dev/null; then
  if [ ! -f "$STOP" ]; then
    echo "GPU ${GPU_T}C >= ${GPU_STOP}C" > "$STOP"
    echo "$(ts) СТОП: перегрів GPU ${GPU_T}C" >> "$LOG"
    tg "🔥 Wallet Radar: ПЕРЕГРІВ GPU ${GPU_T}°C — нічний тест зупинено автоматично."
  fi
  exit 0
fi

# --- мало памʼяті ---
if [ "${MEM_AV:-9999}" -lt "$MEM_MIN_MB" ]; then
  if [ ! -f "$STOP" ]; then
    echo "RAM_avail=${MEM_AV}MB" > "$STOP"
    echo "$(ts) СТОП: мало памʼяті ${MEM_AV}MB" >> "$LOG"
    tg "⚠️ Wallet Radar: вільної памʼяті ${MEM_AV}МБ — нічний тест зупинено, щоб не впав прод."
  fi
  exit 0
fi

# --- попередження ---
if [ "${GPU_T%.*}" -ge "$GPU_WARN" ] 2>/dev/null; then
  echo "$(ts) УВАГА: GPU ${GPU_T}C" >> "$LOG"
fi

# --- відпустити стоп, коли охололо (гістерезис 10 градусів) ---
if [ -f "$STOP" ] && [ "${GPU_T%.*}" -lt "$((GPU_STOP-10))" ] 2>/dev/null \
   && [ "${MEM_AV:-0}" -gt "$((MEM_MIN_MB+300))" ]; then
  rm -f "$STOP"
  echo "$(ts) СТОП знято: GPU ${GPU_T}C, RAM ${MEM_AV}MB" >> "$LOG"
  tg "✅ Wallet Radar: температура в нормі (GPU ${GPU_T}°C), тест відновлено."
fi
