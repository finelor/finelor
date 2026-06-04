# syntax=docker/dockerfile:1.7

#############################
####    BUILDER STAGE    ####
#############################

FROM rust:1.95.0-bookworm AS builder
WORKDIR /app

# Install system tools needed by Rust crates and cargo-leptos.
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config ca-certificates wget \
    && rm -rf /var/lib/apt/lists/*

# Install cargo-binstall, which makes it easier to install other cargo extensions like cargo-leptos
RUN wget https://github.com/cargo-bins/cargo-binstall/releases/latest/download/cargo-binstall-x86_64-unknown-linux-musl.tgz \
  && tar -xvf cargo-binstall-x86_64-unknown-linux-musl.tgz \
  && cp cargo-binstall /usr/local/cargo/bin

# Install the WASM target and the cargo-leptos subcommand.
RUN rustup target add wasm32-unknown-unknown \
    && cargo binstall cargo-leptos -y

COPY . .

# Cache Cargo outputs across Docker builds so small source changes rebuild faster.
RUN --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git/db \
    cargo leptos build --release

# Move the generated outputs out of the cache mount so later stages can copy them.
RUN --mount=type=cache,target=/app/target \
    mkdir -p /artifacts \
    && cp /app/target/release/finelor /artifacts/finelor \
    && cp -a /app/target/site /artifacts/site



#############################
####    RUNTIME STAGE    ####
#############################

FROM debian:bookworm-slim AS runtime
LABEL org.opencontainers.image.source=https://github.com/finelor/finelor

WORKDIR /app

# Install runtime CA certificates and wget for local health checks.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates wget \
    && rm -rf /var/lib/apt/lists/*

# Copy the compiled server binary and Leptos static assets.
COPY --from=builder /artifacts/finelor /usr/local/bin/finelor
COPY --from=builder /artifacts/site ./target/site

# Include runtime files needed when the image is run outside compose mounts.
COPY migrations ./migrations
COPY assets ./assets
COPY config.yaml ./config.yaml

CMD ["finelor"]



##############################
####    HOSTINGER STAGE    ####
##############################

FROM runtime AS hostinger

# Add the Hostinger-specific entrypoint wrapper.
COPY deploy/hostinger/entrypoint.sh /usr/local/bin/finelor-hostinger-entrypoint
RUN chmod 0755 /usr/local/bin/finelor-hostinger-entrypoint

ENTRYPOINT ["finelor-hostinger-entrypoint"]
CMD ["finelor"]
