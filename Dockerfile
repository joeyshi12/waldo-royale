# build stage: wasm module + release server
FROM rust:1-slim AS builder
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown \
    && curl -sSfL https://rustwasm.github.io/wasm-pack/installer/init.sh | sh

WORKDIR /app
COPY . .
RUN wasm-pack build crates/wasm --target web --release --out-dir ../../web/pkg
RUN cargo build --release -p waldo-server

# runtime
FROM debian:bookworm-slim
RUN useradd --system --uid 10001 waldo
WORKDIR /app
COPY --from=builder /app/target/release/waldo-server /usr/local/bin/waldo-server
COPY --from=builder /app/web /app/web

ENV PORT=8080 \
    WALDO_WEB_DIR=/app/web
USER waldo
EXPOSE 8080
CMD ["waldo-server"]
