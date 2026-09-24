# The game is static files: a wasm bundle, a page, and the scenery. There is no
# server, so this image only has to hand those out.
FROM rust:1-slim AS builder
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown \
    && curl -sSfL https://rustwasm.github.io/wasm-pack/installer/init.sh | sh

WORKDIR /app
COPY . .
# Where the client looks for the signalling server. Baked in, because the page may be
# served from anywhere and the signalling server is somewhere else.
ARG SIGNAL_URL=""
ENV SIGNAL_URL=${SIGNAL_URL}
RUN wasm-pack build crates/client --target web --release --out-dir ../../web/pkg

FROM nginx:1-alpine
COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=builder /app/web /usr/share/nginx/html
EXPOSE 8080
