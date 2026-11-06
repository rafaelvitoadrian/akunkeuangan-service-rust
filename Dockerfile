# syntax=docker/dockerfile:1.7
ARG RUST_VERSION=1.99.1
FROM rust:${RUST_VERSION}-slim-bookworm AS build
WORKDIR /build
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential ca-certificates \
    && rm -rf /var/lib/apt/lists/*
ENV CARGO_PROFILE_RELEASE_STRIP=symbols \
    CARGO_PROFILE_RELEASE_LTO=thin \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
    CARGO_PROFILE_RELEASE_OPT_LEVEL=s
COPY Cargo.toml Cargo.lock build.rs ./
COPY migrations ./migrations
COPY src ./src
COPY examples ./examples
COPY tests ./tests
COPY golden ./golden
RUN cargo build --locked --release --no-default-features --features postgres

FROM debian:bookworm-slim AS runtime
ARG VERSION=1.0.0
ARG VCS_REF=unknown
ARG SOURCE=https://github.com/rafaelvitoadrian/akunkeuangan-service-rust
LABEL org.opencontainers.image.title="akunkeuangan-service-rust" \
    org.opencontainers.image.version="$VERSION" \
    org.opencontainers.image.revision="$VCS_REF" \
    org.opencontainers.image.source="$SOURCE" \
    org.opencontainers.image.licenses="MIT"
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
ENV REPO_BACKEND=postgres \
    DATABASE_URL= \
    POSTGRES_AUTO_MIGRATE=true \
    POSTGRES_BOOTSTRAP_USER= \
    PORT=9091 \
    SERVER_PORT=9091 \
    APP_UTC_OFFSET_MINUTES=420 \
    RUST_LOG=info \
    APP_LOG_FILE=
# DATABASE_PASSWORD is intentionally supplied as a runtime Railway Variable.
COPY --from=build /build/target/release/akunkeuangan_service /usr/local/bin/akunkeuangan_service
COPY LICENSE THIRD-PARTY-NOTICES.md /usr/share/doc/akunkeuangan-service-rust/
WORKDIR /app
USER 65532:65532
EXPOSE 9091
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD curl --fail --silent --output /dev/null --max-time 4 "http://127.0.0.1:${PORT:-${SERVER_PORT:-9091}}/health/ready" || exit 1
ENTRYPOINT ["/usr/local/bin/akunkeuangan_service"]
