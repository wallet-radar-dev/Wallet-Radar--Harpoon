#!/usr/bin/env python3
"""Чекання готовності docker-стека перед тим, як вважати застосунок піднятим.

Навіщо: wallet-radar-rust.service має `After=docker.service`, але systemd не чекає,
поки контейнери стануть здоровими. На ребуті застосунок стартує майже одночасно з
docker'ом, і перші ~10–20 секунд він живе:

  * без ключів API (сателіти з Etherscan/Moralis/LunarCrush ще не слухають),
  * без GlitchTip на :9000 — тож помилки, що виникли в цю мить, не долітають
    до Sentry-сумісного сервера (транспорт SDK глухо відкидає подію).

Цей скрипт — ExecStartPost. Він чекає на TCP-порти і ЗАВЖДИ виходить з 0:
ExecStartPost з ненульовим кодом обриває юніт, а це означало б нескінченний
цикл рестартів (Restart=always), якщо якийсь контейнер підніметься пізніше.

Використання: wait-for-stack.py [--timeout 90]
"""

import argparse
import socket
import sys
import time

TARGETS = [
    ("glitchtip-web", "127.0.0.1", 9000),
    ("satellite-a", "127.0.0.1", 8443),
    ("satellite-b", "127.0.0.1", 8444),
]


def probe(host, port, timeout=1.0):
    try:
        with socket.create_connection((host, port), timeout=timeout):
            return True
    except OSError:
        return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=float, default=90.0,
                    help="скільки максимум чекати на ВСІ цілі разом, с")
    args = ap.parse_args()

    deadline = time.monotonic() + args.timeout
    pending = list(TARGETS)
    ready = {}
    started = time.monotonic()

    while pending and time.monotonic() < deadline:
        for name, host, port in list(pending):
            if probe(host, port):
                ready[name] = time.monotonic() - started
                pending.remove((name, host, port))
        if pending:
            time.sleep(0.5)

    for name, host, port in TARGETS:
        if name in ready:
            print(f"[wait-stack] {name} готовий за {ready[name]:.1f}s", flush=True)
        else:
            # Не фатально: канал до сателітів лінивий і сам перепідключиться,
            # а GlitchTip потрібен лише для звітів про помилки.
            print(f"[wait-stack] {name} ({host}:{port}) НЕ піднявся за "
                  f"{args.timeout:.0f}s — ідемо далі", flush=True)

    # Завжди 0: не можемо провалити старт юніта через це очікування.
    return 0


if __name__ == "__main__":
    sys.exit(main())
