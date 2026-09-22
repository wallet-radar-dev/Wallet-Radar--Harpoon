#!/bin/bash
cd /opt/wallet-tracker
TOPIC=$(cat /tmp/council-topic.txt)
./nextaction/target/release/uniproxy council "$TOPIC" --members qwen14b,phi4,deepseek-r1,qwen7b,qwen3b,mistral,llama,gemma --rounds 1 --moderator qwen14b --lang en > /tmp/council-net.out 2>&1
echo COUNCIL_DONE >> /tmp/council-net.out
