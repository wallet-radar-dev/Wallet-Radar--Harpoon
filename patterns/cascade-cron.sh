#!/bin/sh
cd /opt/wallet-tracker/dataset/backfill/local-from-laptop || exit 1
B=/opt/wallet-tracker/patterns/target/release/patterns
$B cascade save=1 >> /opt/wallet-tracker/patterns/cascade-cron.log 2>&1
$B cascalert send=1 >> /opt/wallet-tracker/patterns/cascade-cron.log 2>&1
