#!/usr/bin/env bash
# Правильний порядок коміту й пушу для репозиторію бота.
#
#   tools/push-all.sh "повідомлення коміту"
#
# Що робить (послідовно, без напівкроків):
#   1. перевіряє, що ми в репозиторії бота і на гілці master;
#   2. показує, що зміниться (список файлів і diff --stat);
#   3. комітить (якщо є що комітити) — повідомлення з аргумента;
#   4. пушить у Forgejo (`origin master`) — приватна основна копія;
#   5. пушить у GitHub (`github master:feat/telegram-alerts-bot`) — публічна копія;
#   6. звіряє, що локальний коміт, Forgejo і GitHub — одна й та сама SHA.
#
# Якщо змін немає — кроки 3 пропускаються, решта працює як «синхронізація».
# Якщо push відхилено (захист гілки або розбіжність історії) — скрипт
# зупиняється і каже, що робити.

set -euo pipefail

REPO_DIR="${REPO_DIR:-/root/src/wallet-radar-alerts-bot}"
FORGEJO_BRANCH="${FORGEJO_BRANCH:-master}"
GITHUB_BRANCH="${GITHUB_BRANCH:-feat/telegram-alerts-bot}"
GITHUB_KEY="${GITHUB_KEY:-/root/.ssh/github_key}"
MSG="${1:-}"

fail() { echo "ЗУПИНКА: $*" >&2; exit 1; }
step() { echo; echo "=== $* ==="; }

cd "$REPO_DIR" || fail "немає каталогу $REPO_DIR"
git rev-parse --git-dir >/dev/null 2>&1 || fail "це не git-репозиторій"

step "1. стан репозиторію"
BRANCH=$(git branch --show-current)
[ "$BRANCH" = "master" ] || fail "зараз гілка «$BRANCH» — перейдіть на master (git switch master)"
echo "репозиторій: $REPO_DIR"
echo "гілка: $BRANCH"
git remote -v | sed 's/^/  /'

step "2. що змінено"
if git diff --quiet && git diff --cached --quiet && [ -z "$(git ls-files --others --exclude-standard)" ]; then
  echo "змін немає — просто синхронізую обидва ремоти"
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

step "4. пуш у Forgejo (origin, приватний)"
git push origin "$FORGEJO_BRANCH"

step "5. пуш у GitHub (публічна гілка)"
if [ -f "$GITHUB_KEY" ]; then
  export GIT_SSH_COMMAND="ssh -i $GITHUB_KEY -o IdentitiesOnly=yes"
fi
git push github "master:$GITHUB_BRANCH" || fail "GitHub відхилив пуш. Перевірте: 1) існує репо/гілка, 2) у Settings → Branches не ввімкнено захист гілки, 3) ваш ключ додано в Deploy keys. Повторити примусово: git push --force-with-lease github master:$GITHUB_BRANCH"

step "6. звірка"
LOCAL=$(git rev-parse master)
# Forgejo — своїм ключем (перевіряємо ДО встановлення ключа GitHub).
FORGEJO=$(env -u GIT_SSH_COMMAND git ls-remote origin "refs/heads/$FORGEJO_BRANCH" | cut -f1)
# GitHub — ключем деплою.
GITHUB=$(GIT_SSH_COMMAND="ssh -i $GITHUB_KEY -o IdentitiesOnly=yes" \
  git ls-remote github "refs/heads/$GITHUB_BRANCH" | cut -f1)
echo "локально:  ${LOCAL:0:12}"
echo "Forgejo:   ${FORGEJO:0:12}"
echo "GitHub:    ${GITHUB:0:12}"
if [ "$LOCAL" = "$FORGEJO" ] && [ "$LOCAL" = "$GITHUB" ]; then
  echo "УСІ ТРИ СИНХРОННІ"
else
  echo "РОЗБІЖНІСТЬ — щось не доїхало"
  exit 1
fi
