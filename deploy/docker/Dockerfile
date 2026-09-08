# syntax=docker/dockerfile:1
#
# Multi-stage build for the `rustproxy` binary.
# Stage 1 compiles the release binary; stage 2 produces a slim runtime image.

############ Builder ############
FROM rust:1-bookworm AS builder

WORKDIR /build

# Copy the whole workspace so Cargo can resolve the manifest graph, then
# download all dependencies ahead of the compile step.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations

RUN cargo fetch

RUN cargo build --release -p rustproxy

############ Runtime ############
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/rustproxy /usr/local/bin/rustproxy

# A non-root user to run the service.
RUN useradd --create-home --uid 10001 appuser \
    && mkdir -p /data \
    && chown appuser:appuser /data

# Entrypoint prepares writable state (named volumes are mounted as root) and
# then drops privileges to `appuser` via runuser (provided by util-linux). The
# container starts as root so the entrypoint can chown, and the entrypoint
# execs the provided command as `appuser`.
COPY deploy/docker/entrypoint.sh /usr/local/bin/rustproxy-entrypoint
RUN chmod +x /usr/local/bin/rustproxy-entrypoint

ENV RUST_LOG=rustproxy=info

EXPOSE 8080 1080 20001 19090

ENTRYPOINT ["/usr/local/bin/rustproxy-entrypoint", "/usr/local/bin/rustproxy"]
