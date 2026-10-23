# syntax=docker/dockerfile:1
# Written by `sofie build --image`, which never rewrites it; build with
# `sofie build --image` or `docker build --file <this file> <workspace root>`.
#
# Pin both images before the first build: replace each <digest> with the
# digest `docker buildx imagetools inspect <image>` prints, so every rebuild
# uses the same toolchain and runtime. `sofie build --image` refuses until
# both are pinned.
FROM rust:1-bookworm@sha256:<digest> AS builder
WORKDIR /build
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --profile dist --locked --package shop \
    && cp target/dist/shop /shop

# glibc, libgcc, CA certificates and tzdata (/usr/share/zoneinfo, which
# time zone values read); no shell and no package manager.
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:<digest>
COPY --from=builder /shop /app/shop
USER nonroot:nonroot
ENV SOFIE_ADDRESS=0.0.0.0:8080
EXPOSE 8080
# No HEALTHCHECK: without a shell or an HTTP client the image cannot run one.
# Probe GET /live for liveness and GET /ready for readiness instead. The
# binary writes nothing to disk, so it runs with `docker run --read-only`.
ENTRYPOINT ["/app/shop"]
CMD ["serve"]
