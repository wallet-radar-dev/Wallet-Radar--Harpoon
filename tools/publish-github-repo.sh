#!/usr/bin/env bash
# Створює окремий приватний репозиторій бота на GitHub і заливає в нього код.
#
#   GITHUB_TOKEN=ghp_xxx tools/publish-github-repo.sh
#   або
#   tools/publish-github-repo.sh ghp_xxx
#
# Токен — classic PAT із scope `repo` (або fine-grained із правом
# «Administration: write» на репозиторій). GitHub не дозволяє створити репо
# звичайним SSH-ключем — для цього потрібен саме API-токен.
#
# Що робить:
#   1. створює приватне репо <owner>/wallet-radar-alerts-bot (якщо ще немає);
#   2. додає його remote `github-repo`;
#   3. пушить гілку master;
#   4. звіряє SHA.
#
# Токен ніде не друкується і не потрапляє в історію git.

set -euo pipefail

REPO_DIR="${REPO_DIR:-/root/src/wallet-radar-alerts-bot}"
OWNER="${OWNER:-Vitalii-Vedana}"
REPO_NAME="${REPO_NAME:-wallet-radar-alerts-bot}"
VISIBILITY="${VISIBILITY:-private}"
KEY="${GITHUB_KEY:-/root/.ssh/github_key}"

TOKEN="${GITHUB_TOKEN:-${1:-}}"
[ -n "$TOKEN" ] || { echo "ЗУПИНКА: потрібен токен. Використайте: GITHUB_TOKEN=ghp_xxx $0" >&2; exit 1; }

fail() { echo "ЗУПИНКА: $*" >&2; exit 1; }
step() { echo; echo "=== $* ==="; }

cd "$REPO_DIR" || fail "немає каталогу $REPO_DIR"
git rev-parse --git-dir >/dev/null 2>&1 || fail "це не git-репозиторій"

step "1. чи є вже таке репо"
if curl -s -o /dev/null -w "%{http_code}" -H "Authorization: Bearer $TOKEN" \
     "https://api.github.com/repos/$OWNER/$REPO_NAME" | grep -q "^200$"; then
  echo "репо $OWNER/$REPO_NAME вже існує — створювати не треба"
else
  echo "створюю $OWNER/$REPO_NAME ($VISIBILITY)…"
  code=$(curl -s -o /tmp/create-repo.json -w "%{http_code}" -X POST \
    -H "Authorization: Bearer $TOKEN" \
    -H "Accept: application/vnd.github+json" \
    https://api.github.com/user/repos \
    -d "{\"name\":\"$REPO_NAME\",\"private\":$([ "$VISIBILITY" = private ] && echo true || echo false),\"description\":\"Telegram-бот Wallet Radar: алерти, дайджести, сховище Postgres\",\"auto_init\":false}")
  case "$code" in
    201) echo "створено" ;;
    422) echo "репо вже існує (422) — продовжую" ;;
    *)  echo "API відповів $code:"; head -c 400 /tmp/create-repo.json; echo; exit 1 ;;
  esac
fi

step "2. remote"
git remote remove github-repo 2>/dev/null || true
git remote add github-repo "git@github.com:$OWNER/$REPO_NAME.git"
git remote -v | grep github-repo | sed 's/^/  /'

step "3. пуш"
export GIT_SSH_COMMAND="ssh -i $KEY -o IdentitiesOnly=yes"
git push github-repo master:refs/heads/master || fail "пуш не пройшов — перевірте, чи редоступний ключ $KEY"

step "4. звірка"
LOCAL=$(git rev-parse master)
REMOTE=$(GIT_SSH_COMMAND="ssh -i $KEY -o IdentitiesOnly=yes" \
  git ls-remote github-repo refs/heads/master | cut -f1)
echo "локально: ${LOCAL:0:12}"
echo "GitHub:   ${REMOTE:0:12}"
[ "$LOCAL" = "$REMOTE" ] && echo "СИНХРОННО" || { echo "РОЗБІЖНІСТЬ"; exit 1; }

echo
echo "Готово: https://github.com/$OWNER/$REPO_NAME"
echo "Токен можна видалити — він не зберігається аніде."
