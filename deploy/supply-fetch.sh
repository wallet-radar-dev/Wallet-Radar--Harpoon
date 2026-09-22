#!/usr/bin/env bash
# Фетч total_supply токенів з Blockscout -> dataset/nextaction/supply.tsv.
# supply_tokens = total_supply / 10^decimals. Для supply-нормалізації накопичення.
set -uo pipefail
ND=/opt/wallet-tracker/dataset/nextaction
OUT=$ND/supply.tsv
LOG=/opt/wallet-tracker/deploy/supply-fetch.log

host(){
  case "$1" in
    eth) echo https://eth.blockscout.com;;
    base) echo https://base.blockscout.com;;
    optimism) echo https://optimism.blockscout.com;;
    gnosis) echo https://gnosis.blockscout.com;;
    polygon) echo https://polygon.blockscout.com;;
    arbitrum) echo https://arbitrum.blockscout.com;;
    zksync) echo https://zksync.blockscout.com;;
    scroll) echo https://scroll.blockscout.com;;
    mode) echo https://explorer.mode.network;;
    fraxtal) echo https://fraxscan.blockscout.com;;
    blast) echo https://blast.blockscout.com;;
    linea) echo https://explorer.linea.build;;
    zora) echo https://explorer.zora.energy;;
    celo) echo https://celo.blockscout.com;;
    mantle) echo https://mantle.blockscout.com;;
    sonic) echo https://sonic.blockscout.com;;
    rootstock) echo https://rootstock.blockscout.com;;
    fuse) echo https://explorer.fuse.io;;
    metis) echo https://andromeda-explorer.metis.io;;
    aurora) echo https://explorer.aurora.dev;;
    lisk) echo https://blockscout.lisk.com;;
    redstone) echo https://explorer.redstone.xyz;;
    immutable) echo https://explorer.immutable.com;;
    lukso) echo https://explorer.execution.mainnet.lukso.network;;
    *) echo "";;
  esac
}

printf "chain\taddress\tsupply_tokens\tsymbol\n" > "$OUT"
total=0; ok=0; miss=0
for f in "$ND"/prices/*.json; do
  b=$(basename "$f" .json)
  chain="${b%%_*}"; addr="${b#*_}"
  h=$(host "$chain"); [ -z "$h" ] && { miss=$((miss+1)); continue; }
  total=$((total+1))
  r=$(curl -s --max-time 20 "$h/api/v2/tokens/$addr" 2>/dev/null)
  ts=$(echo "$r" | jq -r ".total_supply // empty" 2>/dev/null)
  dec=$(echo "$r" | jq -r ".decimals // \"18\"" 2>/dev/null)
  sym=$(echo "$r" | jq -r ".symbol // \"\"" 2>/dev/null | tr -d "\t\n")
  if [ -n "$ts" ] && [ "$ts" != "null" ]; then
    supply=$(awk -v t="$ts" -v d="$dec" "BEGIN{printf \"%.6f\", t/(10^d)}")
    printf "%s\t%s\t%s\t%s\n" "$chain" "$addr" "$supply" "$sym" >> "$OUT"
    ok=$((ok+1))
  else
    miss=$((miss+1))
  fi
  sleep 0.15
done
printf "%s supply: total=%s ok=%s miss=%s\n" "$(date -u +%H:%M:%SZ)" "$total" "$ok" "$miss" >> "$LOG"
