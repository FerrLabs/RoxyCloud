FROM rust:1.96-slim-bookworm AS builder
WORKDIR /build
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config \
 && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY api/Cargo.toml api/
COPY app/Cargo.toml app/
COPY cli/Cargo.toml cli/
COPY crates/core/Cargo.toml crates/core/
COPY crates/client/Cargo.toml crates/client/
RUN mkdir -p api/src app/src cli/src crates/core/src crates/client/src \
 && echo 'fn main() {}' | tee api/src/main.rs app/src/main.rs cli/src/main.rs > /dev/null \
 && touch api/src/lib.rs crates/core/src/lib.rs crates/client/src/lib.rs \
 && cargo build --release --locked --package roxycloud-api \
 && rm -rf api/src cli/src crates/core/src crates/client/src
COPY api/src api/src
COPY cli/src cli/src
COPY crates crates
COPY api/migrations api/migrations
RUN find api/src cli/src crates -name '*.rs' -exec touch {} + \
 && cargo build --release --locked --package roxycloud-api

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --create-home --uid 10001 roxy \
 && mkdir -p /var/lib/roxycloud \
 && chown roxy:roxy /var/lib/roxycloud
COPY --from=builder /build/target/release/roxycloud-api /usr/local/bin/roxycloud-api
USER roxy
ENV PORT=3001 BLOB_ROOT=/var/lib/roxycloud
VOLUME /var/lib/roxycloud
EXPOSE 3001
ENTRYPOINT ["/usr/local/bin/roxycloud-api"]
