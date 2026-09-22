#!/usr/bin/env bash
# Шифрований бекап КРИТИЧНОГО (не сирих даних — їх можна докраулити).
# Кладе .tar.zst.gpg у /mnt/nvme-disk/wallet-backups/offsite/ + залишає копію-маркер.
# Розшифрувати:  gpg --batch --passphrase-file <ключ> -d файл.gpg | zstd -d | tar -xf -
set -euo pipefail

ROOT=/opt/wallet-tracker
OUT=/mnt/nvme-disk/wallet-backups/offsite
PASSFILE=/root/.wr-backup-pass
KEEP=7
STAMP=$(date +%Y%m%d-%H%M)
NAME="wr-critical-${STAMP}.tar.zst.gpg"

mkdir -p "$OUT"

if [[ ! -f "$PASSFILE" ]]; then
  echo "ПОМИЛКА: немає ключа $PASSFILE" >&2
  exit 1
fi

# Що входить: секрети, код, конфіги розгортання, вітрини, метадані краулу.
# Що НЕ входить: target/, node_modules/, сирі jsonl/csv (81 ГБ, відтворювані краулом).
tar -C / \
    --exclude='*/target' \
    --exclude='*/node_modules' \
    --exclude='*.tar.zst.gpg' \
    -cf - \
    opt/wallet-tracker/.env \
    opt/wallet-tracker/dataset \
    opt/wallet-tracker/deploy \
    opt/wallet-tracker/rust-server/src \
    opt/wallet-tracker/backfill/src \
    opt/wallet-tracker/nextaction/src \
    opt/wallet-tracker/patterns/src \
    opt/wallet-tracker/features/src \
    opt/wallet-tracker/satellite/src \
    opt/wallet-tracker/wr-dict/src \
    opt/wallet-tracker/gat/src \
    opt/wallet-tracker/web \
    opt/wallet-tracker/proto \
    opt/wallet-tracker/scripts \
    opt/wallet-tracker/.git \
    etc/systemd/system/wallet-radar-rust.service \
    etc/systemd/system/wallet-diskguard.service \
    2>/dev/null \
  | zstd -T0 -9 -q \
  | gpg --batch --yes --symmetric --cipher-algo AES256 \
        --passphrase-file "$PASSFILE" -o "$OUT/$NAME"

# метадані краулу — окремо (великий, але критичний для відновлення прогресу)
cp -f /mnt/nvme-disk/wallet-backfill/_progress.csv "$OUT/_progress-${STAMP}.csv" 2>/dev/null || true
gzip -f "$OUT/_progress-${STAMP}.csv" 2>/dev/null || true

# перелік cron окремо (для швидкого відновлення розкладу)
crontab -l -u ubuntu > "$OUT/crontab-ubuntu-${STAMP}.txt" 2>/dev/null || true

# ротація
ls -1t "$OUT"/wr-critical-*.tar.zst.gpg 2>/dev/null | tail -n +$((KEEP+1)) | xargs -r rm -f
ls -1t "$OUT"/_progress-*.csv.gz        2>/dev/null | tail -n +$((KEEP+1)) | xargs -r rm -f
ls -1t "$OUT"/crontab-ubuntu-*.txt      2>/dev/null | tail -n +$((KEEP+1)) | xargs -r rm -f

SIZE=$(du -h "$OUT/$NAME" | cut -f1)
echo "[$(date '+%F %T')] OK  $NAME  ($SIZE)"
echo "ВИВАНТАЖИТИ ПОЗА ПРИМІЩЕННЯ: $OUT/$NAME"
