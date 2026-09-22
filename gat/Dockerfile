# Навчання GAT. Раніше тут стояв python:3.11 + torch + torch_geometric — кілька гігабайтів заради
# графа з 1891 вузла. Тепер це один Rust-бінарник на candle: та сама GAT, ті самі обчислення,
# важкого стека не лишилося.
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=build /src/target/release/gat /usr/local/bin/gat
# Дані монтуються в /data (том із dataset/).
ENV DATA=/data
ENTRYPOINT ["/usr/local/bin/gat"]
