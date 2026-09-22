#!/bin/bash
cd /opt/wallet-tracker
CHAIN=8453 API_BASE=https://base.blockscout.com/api CONC=10 ./backfill/target/release/eth-edges2 /opt/wallet-tracker dataset/backfill/net-members.txt > /tmp/l2-8453.log 2>&1
echo BASE_DONE >> /tmp/l2-progress.log
CHAIN=324 API_BASE=https://zksync.blockscout.com/api CONC=10 ./backfill/target/release/eth-edges2 /opt/wallet-tracker dataset/backfill/net-members.txt > /tmp/l2-324.log 2>&1
echo L2_ALL_DONE >> /tmp/l2-progress.log
