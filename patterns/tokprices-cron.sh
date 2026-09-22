#!/bin/bash
cd /opt/wallet-tracker/backfill || exit 1
export PATH=$PATH:/home/ubuntu/.cargo/bin
timeout 900 ./target/release/tokprices /opt/wallet-tracker >> /opt/wallet-tracker/backfill/tokprices.cron.log 2>&1
