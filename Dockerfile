# syntax=docker/dockerfile:1
FROM --platform=$BUILDPLATFORM rust:1.100.0-bookworm AS builder
ARG BUILDARCH
ARG TARGETARCH

WORKDIR /build

RUN if [ "${TARGETARCH:-amd64}" = "arm64" ] && [ "${BUILDARCH:-amd64}" != "arm64" ]; then \
        apt-get update && \
        apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu libc6-dev-arm64-cross && \
        rm -rf /var/lib/apt/lists/* && \
        rustup target add aarch64-unknown-linux-gnu; \
    fi

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN if [ "${TARGETARCH:-amd64}" = "arm64" ] && [ "${BUILDARCH:-amd64}" != "arm64" ]; then \
        CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
        cargo build --release --locked --target aarch64-unknown-linux-gnu --bin filecleaner && \
        cp target/aarch64-unknown-linux-gnu/release/filecleaner /build/filecleaner && \
        aarch64-linux-gnu-strip /build/filecleaner; \
    else \
        cargo build --release --locked --bin filecleaner && \
        cp target/release/filecleaner /build/filecleaner && \
        strip /build/filecleaner; \
    fi

FROM debian:bookworm-slim AS runtime

LABEL org.opencontainers.image.title="FileCleaner" \
      org.opencontainers.image.description="Deterministic Rust directory retention engine with backup safety guardrails, 6-layer active-file detection, lifecycle hooks, pure-Rust compression, and daemon mode" \
      org.opencontainers.image.source="https://github.com/jmrenouard/FileCleaner" \
      org.opencontainers.image.url="https://hub.docker.com/r/jmrenouard/filecleaner" \
      org.opencontainers.image.licenses="Apache-2.0"

RUN groupadd -g 10001 fcgroup && \
    useradd -u 10001 -g fcgroup -s /usr/sbin/nologin -M -d /app fcuser && \
    mkdir -p /app /data /etc/filecleaner /var/lib/filecleaner /var/log/filecleaner && \
    chown -R fcuser:fcgroup /app /data /etc/filecleaner /var/lib/filecleaner /var/log/filecleaner

WORKDIR /app

COPY --from=builder --chown=fcuser:fcgroup /build/filecleaner /usr/local/bin/filecleaner
COPY --chown=fcuser:fcgroup README.md README.fr.md LICENSE ./
COPY --chown=fcuser:fcgroup examples/ /etc/filecleaner/examples/

USER fcuser
VOLUME ["/data", "/etc/filecleaner", "/var/lib/filecleaner", "/var/log/filecleaner"]
HEALTHCHECK --interval=30s --timeout=5s --retries=1 CMD ["/usr/local/bin/filecleaner", "--version"]
ENTRYPOINT ["/usr/local/bin/filecleaner"]
CMD ["--help"]
