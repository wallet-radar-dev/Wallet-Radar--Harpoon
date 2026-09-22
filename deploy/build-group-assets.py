# -*- coding: utf-8 -*-
"""Побудова зв'язку ГРУПА КАПІТАЛУ → ЇЇ АКТИВИ з часткою емісії.

Навіщо: у capital-groups.json поле assets порожнє у всіх 73 груп — зв'язку не було
взагалі. Фронтенд не мав що показувати, а старий список активів (64 позиції в
capital-group-assets.json) не був прив'язаний до конкретних груп.

Як рахується частка: для кожного активу беремо адреси-холдери з наявних сканів,
визначаємо групу кожної адреси за повним членством (127 573 гаманці, 73 групи),
і сумуємо їхні частки емісії всередині групи.

Обмеження, які треба знати:
  • сума часток може перевищувати 100% — одна адреса потрапляє в кілька сканів
    із різними базами розрахунку. Обрізаємо до 100 і позначаємо прапорцем.
  • покриття неповне: скани охоплюють переважно топ-холдерів, тому активи
    знаходяться не у всіх груп. Це обмеження ДАНИХ, а не логіки.
"""
import json, collections, sys, time

D = "/opt/wallet-tracker/dataset"
MIN_SHARE = 20.0     # основний фокус: група викупила від 20% емісії
FALLBACK_MIN = 1.0   # для груп, що не дотягують до 20% — щоб вони не зникали
FALLBACK_TOP = 12    # скільки їхніх найбільших позицій показати

# ── стейбли, обгортки й фіат-еквіваленти: для стратегій це не альткоїни ──────
STABLE_HINTS = ("USD", "EUR", "GBP", "UAH", "DAI", "TUSD", "FDUSD", "PYUSD", "USDE",
                "USDT", "USDC", "BUSD", "GUSD", "LUSD", "FRAX", "USTB", "BILL")
WRAPPED_HINTS = ("WETH", "WBTC", "STETH", "WSTETH", "RETH", "CBETH", "WEETH",
                 "CBBTC", "TBTC", "WMATIC", "WBNB", "WAVAX")
MAJORS = ("BTC", "ETH", "BNB", "SOL", "XRP", "ADA", "DOGE", "TRX", "TON", "DOT", "MATIC")


def kind_of(sym: str) -> str:
    """altcoin | stable | wrapped | major — визначає, що показувати у Стратегіях."""
    s = (sym or "").upper()
    if any(h in s for h in STABLE_HINTS):
        return "stable"
    if s in WRAPPED_HINTS or any(s == w for w in WRAPPED_HINTS):
        return "wrapped"
    if s in MAJORS:
        return "major"
    return "altcoin"


def load(name):
    with open(f"{D}/{name}", encoding="utf-8") as f:
        return json.load(f)


def main():
    membership = load("backfill/group-membership.json")["wallets"]
    addr2grp = {a.lower(): v["group"] for a, v in membership.items()}

    groups = {g["id"]: g for g in load("capital-groups.json")["groups"]}

    # (group, symbol) -> сума часток; окремо метадані активу
    acc = collections.defaultdict(float)
    holders = collections.defaultdict(set)
    meta = {}

    for src in ("wallet-asset-scan.json", "asset-forensics.json"):
        try:
            assets = load(src)["assets"]
        except Exception as e:
            print(f"  пропущено {src}: {e}", file=sys.stderr)
            continue
        for a in assets:
            sym = a.get("symbol")
            if not sym:
                continue
            prev = meta.get(sym, {})
            meta[sym] = {
                "symbol": sym,
                "contract": a.get("contract") or prev.get("contract"),
                "name": a.get("name") or prev.get("name"),
                "rank": a.get("rank") if a.get("rank") is not None else prev.get("rank"),
                "top10pct": a.get("top10pct") if a.get("top10pct") is not None else prev.get("top10pct"),
                "kind": kind_of(sym),
            }
            for grp in (a.get("groups") or []):
                for m in (grp.get("members") or []):
                    addr = str(m.get("addr", "")).lower()
                    gid = addr2grp.get(addr)
                    if not gid:
                        continue
                    acc[(gid, sym)] += float(m.get("share") or 0)
                    holders[(gid, sym)].add(addr)

    # ── збірка за групами ───────────────────────────────────────────────────
    # Групи, у яких є хоч один актив від MIN_SHARE, беруть лише такі активи.
    # Решта груп показують свій топ — інакше вони зникли б із видачі зовсім.
    strong = {gid for (gid, _), share in acc.items() if share >= MIN_SHARE}

    by_group = collections.defaultdict(list)
    capped = 0
    for (gid, sym), share in acc.items():
        if gid in strong:
            if share < MIN_SHARE:
                continue
        elif share < FALLBACK_MIN:
            continue
        over = share > 100.0
        if over:
            capped += 1
        m = meta.get(sym, {"symbol": sym, "kind": kind_of(sym)})
        by_group[gid].append({
            **m,
            "belowThreshold": gid not in strong,
            "emissionShare": round(min(share, 100.0), 2),
            "shareRaw": round(share, 2),
            "capped": over,
            "holders": len(holders[(gid, sym)]),
        })

    out_groups = []
    for gid, items in by_group.items():
        items.sort(key=lambda x: -x["emissionShare"])   # від найбільшого до найменшого
        if gid not in strong:
            items[:] = items[:FALLBACK_TOP]
        g = groups.get(gid, {})
        alt = [i for i in items if i["kind"] == "altcoin"]
        out_groups.append({
            "id": gid,
            "name": g.get("name"),
            "size": g.get("size"),
            "totalUsd": g.get("totalUsd"),
            "assetsCount": len(items),
            "altcoinsCount": len(alt),
            "topShare": items[0]["emissionShare"] if items else 0,
            "assets": items,
        })
    out_groups.sort(key=lambda x: (-x["topShare"], -x["assetsCount"]))

    total_assets = sum(g["assetsCount"] for g in out_groups)
    result = {
        "generated": int(time.time() * 1000),
        "method": "частка емісії = сума часток адрес групи серед холдерів активу; "
                  "джерела: wallet-asset-scan + asset-forensics; членство: group-membership",
        "minShare": MIN_SHARE,
        "fallbackMin": FALLBACK_MIN,
        "groupsTotal": len(groups),
        "groupsWithAssets": len(out_groups),
        "assetsTotal": total_assets,
        "cappedPairs": capped,
        "caveat": "фокус — активи від 20% емісії; групи без таких позицій показують свій топ із позначкою. Один актив може бути в кількох групах — перетин інтересів не приховується. "
                  "Частки понад 100% обрізано до 100 (адреса трапляється в кількох джерелах).",
        "groups": out_groups,
    }

    with open(f"{D}/group-assets.json", "w", encoding="utf-8") as f:
        json.dump(result, f, ensure_ascii=False)

    print(f"груп із активами: {len(out_groups)} із {len(groups)}")
    print(f"пар група-актив (>= {MIN_SHARE}%): {total_assets}")
    print(f"обрізано понад 100%: {capped}")
    for g in out_groups[:6]:
        print(f"  {g['id']:10} активів {g['assetsCount']:4}  альткоїнів {g['altcoinsCount']:4}  "
              f"топ {g['topShare']}%")


if __name__ == "__main__":
    main()
