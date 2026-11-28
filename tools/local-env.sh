#!/bin/bash
# Завантаження локальних значень, які НЕ тримаємо в Git.
#
# Публічний репозиторій не має містити внутрішніх адрес, імен власника
# приватних репозиторіїв чи локальних імен репозиторіїв. Робочі значення
# лежать поза Git:
#
#   1) /etc/wallet-radar/local.env                      (0600, root)
#   2) ~/.config/wallet-radar/local.env                 (0600, користувач)
#
# Значення зі змінних оточення мають найвищий пріоритет: їх можна задати
# інлайн у systemd-юніті або в конкретному запуску.
#
# Використання:
#   . "$(dirname "$0")/local-env.sh"
#   wr_load_local_env
#   wr_require EXPECT_IP ROUTER REPO || exit 1
#
# Скрипти нижче не мають власних «внутрішніх» дефолтів: якщо значення не
# задано — вони кажуть про це голосно, замість мовчки вгадувати адресу.

# Файли-джерела за замовчуванням (перекривається через WR_LOCAL_ENV_FILES).
WR_LOCAL_ENV_FILES_DEFAULT="/etc/wallet-radar/local.env ${HOME:-/root}/.config/wallet-radar/local.env"

# Завантажити env-файли (усі доступні, пізніші перекривають попередні).
wr_load_local_env() {
    local f
    for f in ${WR_LOCAL_ENV_FILES:-${WR_LOCAL_ENV_FILES_DEFAULT}}; do
        [ -r "$f" ] && . "$f"
    done
    return 0
}

# Перевірити, що всі перелічені змінні задано; інакше — зрозуміла помилка.
wr_require() {
    local v missing=""
    for v in "$@"; do
        [ -z "${!v:-}" ] && missing="${missing} ${v}"
    done
    if [ -n "$missing" ]; then
        echo "ПОМИЛКА: не задано змінних:${missing}" >&2
        echo "Задай їх у /etc/wallet-radar/local.env (файл поза Git) або експортуй у середовище." >&2
        return 1
    fi
    return 0
}
