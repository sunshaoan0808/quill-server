# Kaleido headless server + bundled SPA
# Build with rust 1.86+ preferred; 1.85 works if idna/icu stack resolves.
# [refactor] 旧镜像只拷 crates、不含前端产物——容器内 /web/ 404。现在增加
# node 构建阶段（npm run build:vite 输出 web/assets/app.js + styles.css 并
# stamp web/index.html），最终镜像内以 KALEIDO_WEB_DIR 显式指向 /app/web。
FROM node:20-bookworm-slim AS webbuilder
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY build.js vite.config.mjs ./
COPY scripts ./scripts
COPY src ./src
COPY web ./web
RUN npm run build:vite

FROM rust:1.86-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY crates ./crates
RUN cargo build --release -p kaleido-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home kaleido
WORKDIR /app
COPY --from=builder /app/target/release/kaleido-server /usr/local/bin/kaleido-server
COPY --from=webbuilder /app/web /app/web
ENV KALEIDO_HOST=0.0.0.0
ENV KALEIDO_PORT=18766
ENV KALEIDO_DATA=/data
ENV KALEIDO_WEB_DIR=/app/web
EXPOSE 18766
VOLUME ["/data"]
# 命名卷首次挂载时若镜像内已存在该目录，Docker 会沿用镜像内属主；
# 预创建并 chown，避免 uid 10001 的服务端对 /data 写入 EACCES。
RUN mkdir -p /data && chown kaleido:kaleido /data
USER kaleido
ENTRYPOINT ["/usr/local/bin/kaleido-server"]
