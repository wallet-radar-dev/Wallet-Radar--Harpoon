# Контекст збірки — КОРІНЬ репо: сателіту потрібен ../proto/satellite.proto.
FROM rust:1-slim-bookworm AS build
WORKDIR /src
# tonic-build генерує код із .proto → потрібен protoc.
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY proto ./proto
COPY satellite ./satellite
WORKDIR /src/satellite
RUN cargo build --release

FROM debian:bookworm-slim
# ca-certificates обов'язкові: сателіт сам ходить по HTTPS у Etherscan/Moralis/…,
# і без кореневих сертифікатів кожен такий запит впаде на перевірці TLS.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/satellite/target/release/satellite /usr/local/bin/satellite
# Ключі приходять з оточення, сертифікати монтуються в /pki — в образі немає ні того, ні того.
ENTRYPOINT ["/usr/local/bin/satellite"]
