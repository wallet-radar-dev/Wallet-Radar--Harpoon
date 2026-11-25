#!/usr/bin/env bash
# Правильний порядок коміту й пушу для репозиторію бота.
#
#   tools/push-all.sh "повідомлення коміту"
#
# Що робить (послідовно, без напівкроків):
#   1. перевіряє, що ми в репозиторії бота і на гілці master;
#   2. показує, що зміниться (список файлів і diff --stat);
#   3. комітить (якщо є що комітити) — повідомлення з аргумента;
#   4. пушить у Forgejo (`origin master`) — приватна робоча копія, джерело правок;
#   5. пушить у публічний GitHub: бере `main` публічного репо, накладає на нього
#      стан бота і пушить це гілкою `bot/sync-<час>` — гілка готова до PR;
#   6. звіряє, що локальний коміт і Forgejo — одна й та сама SHA.
#
# Чому не «push master:master» на GitHub: історії приватного репо (Forgejo) і
# публічного (GitHub) різні — там не спільний корінь. Код бота потрапляє в
# публічний репо не копією SHA, а накладанням стану на `main`, після чого
# гілка йде в PR (PR #13 «Feat/telegram alerts bot» злито саме так).
#
# Якщо змін немає — крок 3 пропускається, решта працює як «синхронізація».
set -euo pipefail

REPO_DIR="${REPO_DIR:-/root/src/wallet-radar-alerts-bot}"
FORGEJO_BRANCH="${FORGEJO_BRANCH:-master}"
GITHUB_BRANCH="${GITHUB_BRANCH:-main}"
GITHUB_REPO="${GITHUB_REPO:-wallet-radar-dev/Wallet-Radar--Harpoon}"
GITHUB_KEY="${GITHUB_KEY:-/root/.ssh/github_key}"
SYNC_PREFIX="${SYNC_PREFIX:-bot/sync}"
MSG="${1:-}"

# Файли, які належать боту. У публічному репо вони лежать за тими самими
# шляхами; окремий виняток — README (у публічному репо це README-alerts-bot.md,
# бо головна сторінка належить проєкту Wallet Radar).
BOT_PATHS=(
    bot
    crates/admin
    crates/alert-engine
    crates/broadcast
    crates/privacy-filter
    crates/storage
    crates/telegram-gateway
    crates/user-session
    crates/wr-client
    deploy/logind
    deploy/wallet-radar-alerts-bot.service
    deploy/wra-fix-remote.service
    deploy/wra-fix-remote.timer
    deploy/wra-postboot-check.service
    docs/ARCHITECTURE.md
    docs/DESIGN.md
    tools/fix-remote-after-ip-change.sh
    tools/live_smoke.sh
    tools/postboot-check.sh
    tools/postgres_setup.sh
    tools/publish-github-repo.sh
    tools/push-all.sh
    tools/reserve-ip-on-router.sh
    tools/watch-and-fix-remote.sh
    config.example.toml
    Cargo.lock
    Cargo.toml
)

fail() { echo "ЗУПИНКА: $*" >&2; exit 1; }
step() { echo; echo "=== $* ==="; }

cd "$REPO_DIR" || fail "немає каталогу $REPO_DIR"
git rev-parse --git-dir >/dev/null 2>&1 || fail "це не git-репозиторій"

step "1. стан репозиторію"
BRANCH=$(git branch --show-current)
[ "$BRANCH" = "$FORGEJO_BRANCH" ] || fail "зараз гілка «$BRANCH» — перейдіть на $FORGEJO_BRANCH (git switch $FORGEJO_BRANCH)"
echo "репозиторій: $REPO_DIR"
echo "гілка: $BRANCH"
git remote -v | sed 's/^/  /'

step "2. що змінено"
if git diff --quiet && git diff --cached --quiet && [ -z "$(git ls-files --others --exclude-standard)" ]; then
    echo "змін немає — просто синхронізуємо обидва ремоти"
else
    git status --short | sed 's/^/  /'
    echo "--- diff --stat ---"
    git diff --stat | sed 's/^/  /'
    git diff --cached --stat | sed 's/^/  /'
    [ -n "$MSG" ] || fail "є зміни, але немає повідомлення коміту — викличте: tools/push-all.sh \"текст\""
fi

step "3. коміт"
if [ -n "$MSG" ]; then
    git add -A
    git commit -q -m "$MSG"
    echo "коміт: $(git log --oneline -1)"
else
    echo "немає змін — коміт не потрібен"
fi

step "4. пуш у Forgejo (origin, приватна робоча копія)"
git push origin "$FORGEJO_BRANCH"

step "5. пуш у публічний GitHub (гілка від main, готова до PR)"
if [ -f "$GITHUB_KEY" ]; then
    export GIT_SSH_COMMAND="ssh -i $GITHUB_KEY -o IdentitiesOnly=yes"
fi
git fetch --quiet github "$GITHUB_BRANCH" || fail "не вдалося забрати $GITHUB_BRANCH із GitHub"
WORK="$(mktemp -d)/pub"
cleanup() {
    git worktree remove --force "$WORK" >/dev/null 2>&1 || true
    rm -rf "$(dirname "$WORK")" >/dev/null 2>&1 || true
}
trap cleanup EXIT

git worktree add --quiet --detach "$WORK" "github/$GITHUB_BRANCH"

# Накладаємо стан бота з master на main публічного репо.
git -C "$WORK" checkout master -- "${BOT_PATHS[@]}"
# README бота у публічному репо живе окремим файлом.
git show master:README.md >"$WORK/README-alerts-bot.md"
git -C "$WORK" add README-alerts-bot.md

# Файли, які бот видалив, але які лишилися в main, теж прибираємо.
while IFS= read -r f; do
    if ! git cat-file -e "master:$f" 2>/dev/null; then
        echo "  видаляю (бот більше не має): $f"
        git -C "$WORK" rm -q --ignore-unmatch "$f" >/dev/null 2>&1 || true
    fi
done < <(git ls-tree -r --name-only "github/$GITHUB_BRANCH" -- "${BOT_PATHS[@]}")

# .gitignore у публічному репо — надмножина (там ще й правила проєкту),
# тому переносимо тільки перевірку: правила бота мають бути на місці.
missing=$(comm -23 \
    <(grep -vE '^\s*(#|$)' .gitignore | sort -u) \
    <(git show "github/$GITHUB_BRANCH:.gitignore" | grep -vE '^\s*(#|$)' | sort -u))
if [ -n "$missing" ]; then
    echo "УВАГА: у .gitignore публічного репо немає правил бота:"
    printf '%s\n' "$missing" | sed 's/^/  /'
fi

git -C "$WORK" add -A
if git -C "$WORK" diff --cached --quiet; then
    echo "код бота в GitHub ($GITHUB_BRANCH) уже актуальний — пушити нічого"
else
    git -C "$WORK" -c user.name="$(git config user.name || echo 'Wallet Radar Bot')" \
        -c user.email="$(git config user.email || echo 'bot@wallet-radar.local')" \
        commit -q -m "Бот: ${MSG:-синхронізація з приватною копією}"
    SYNC_BRANCH="${SYNC_PREFIX}-$(date +%Y%m%d-%H%M%S)"
    git -C "$WORK" push -q github "HEAD:refs/heads/$SYNC_BRANCH"
    echo "гілка запушена: $SYNC_BRANCH"
    echo "створи PR: https://github.com/$GITHUB_REPO/compare/$GITHUB_BRANCH...$SYNC_BRANCH"
fi

step "6. звірка"
LOCAL=$(git rev-parse master)
# Forgejo — своїм ключем (перевіряємо ДО встановлення ключа GitHub).
FORGEJO=$(env -u GIT_SSH_COMMAND git ls-remote origin "refs/heads/$FORGEJO_BRANCH" | cut -f1)
echo "локально:  ${LOCAL:0:12}"
echo "Forgejo:   ${FORGEJO:0:12}"
if [ "$LOCAL" = "$FORGEJO" ]; then
    echo "Forgejo СИНХРОННИЙ"
else
    echo "РОЗБІЖНІСТЬ — Forgejo не прийняв пуш"
    exit 1
fi
