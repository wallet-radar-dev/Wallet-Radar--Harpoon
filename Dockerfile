# ---- Стадія збірки: ставимо залежності й компілюємо TypeScript (web/app.ts → app.js) ----
FROM node:22-alpine AS build
WORKDIR /app
COPY package*.json ./
RUN npm install --no-audit --no-fund
COPY tsconfig.json ./
COPY web ./web
RUN npx --yes -p typescript@5 tsc -p tsconfig.json

# ---- Стадія запуску: лише потрібне для роботи сервера ----
FROM node:22-alpine
WORKDIR /app
ENV NODE_ENV=production
ENV PORT=3000
ENV DATA_DIR=/data
# залежності та зібраний фронтенд беремо зі стадії build
COPY --from=build /app/node_modules ./node_modules
COPY package*.json ./
COPY server ./server
COPY dataset ./dataset
COPY --from=build /app/web ./web
RUN mkdir -p /data
EXPOSE 3000
CMD ["node", "server/index.js"]
