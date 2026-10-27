# Кор Wallet Radar: Rust-сервер + зібраний фронтенд. Контекст збірки — КОРІНЬ репо.
# Раніше тут була Node-збірка (server/ і dataset/ у публічному репо немає, вона не збиралась).
# Запуск усього стеку: `docker compose up --build` (див. docker-compose.yml, README).

# ---- Фронтенд: web/src/*.ts + tailwind → web/app.js, web/app.css ----
FROM node:22-alpine AS web
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY tsconfig.json tailwind.config.cjs ./
COPY scripts ./scripts
COPY web ./web
# Без `npm run typecheck`: на Develop tsc зараз червоний (web/src/graph.ts: невикористані змінні,
# possibly-undefined), а для образу потрібен лише бандл. Типи перевіряє CI, не збірка образу.
RUN npm run build:js && npm run build:css && node scripts/stamp-assets.mjs

# ---- Rust: rust-server і wr-import (наповнення довідників) ----
FROM rust:1-slim-bookworm AS rust
# tonic-build генерує gRPC-код із proto/satellite.proto → потрібен protoc.
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY proto ./proto
COPY wr-store ./wr-store
COPY rust-server ./rust-server
# Кеш реєстру й target між збірками (BuildKit): без нього кожна зміна коду тягне ~500 крейтів наново.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/rust-server/target \
    --mount=type=cache,target=/src/wr-store/target \
    cargo build --release --manifest-path rust-server/Cargo.toml \
    && cargo build --release --manifest-path wr-store/Cargo.toml --bin wr-import \
    && cp rust-server/target/release/rust-server /rust-server \
    && cp wr-store/target/release/wr-import /wr-import

# ---- Запуск ----
FROM debian:bookworm-slim
# ca-certificates: кор ходить по HTTPS (rustls + webpki-roots, але й системні корені не завадять).
# curl — лише для healthcheck.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=rust /rust-server /usr/local/bin/rust-server
COPY --from=rust /wr-import /usr/local/bin/wr-import
COPY --from=web /app/web ./web
COPY deploy/docker/store-init.sh /usr/local/bin/store-init.sh
# sed: на Windows із autocrlf скрипт приїжджає з CRLF, і ядро не знаходить інтерпретатор /bin/sh.
RUN sed -i 's/\r$//' /usr/local/bin/store-init.sh \
    && chmod +x /usr/local/bin/store-init.sh && mkdir -p /app/dataset /app/store /app/data

# APP_ROOT=/app: сервер читає /app/dataset, /app/web і /app/.env звідси (за замовчуванням ".." — не наш випадок).
ENV APP_ROOT=/app \
    RUST_PORT=3000 \
    DATA_DIR=/app/data \
    WR_DB=/app/store/wallet-radar.db \
    WR_SEED=/app/seed \
    WR_REPO=/app \
    PKI_DIR=/pki
EXPOSE 3000
CMD ["rust-server"]
