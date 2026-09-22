#!/bin/sh
cd /opt/wallet-tracker/dataset/backfill/local-from-laptop || exit 1
BIN=/opt/wallet-tracker/patterns/target/release/patterns
$BIN mmsignal save=1 >> /opt/wallet-tracker/patterns/mmsignal-cron.log 2>&1
$BIN mmalert send=1 >> /opt/wallet-tracker/patterns/mmsignal-cron.log 2>&1
