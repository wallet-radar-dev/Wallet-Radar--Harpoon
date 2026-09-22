#!/bin/bash
cd /opt/wallet-tracker
for C in 42161 8453 10 324; do
  CHAIN=$C CONC=16 ./backfill/target/release/eth-edges2 /opt/wallet-tracker /opt/wallet-tracker/dataset/backfill/net-members.txt > /tmp/l2-$C.log 2>&1
  echo "CHAIN_$C_DONE" >> /tmp/l2-progress.log
done
echo L2_ALL_DONE >> /tmp/l2-progress.log
