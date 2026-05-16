# syntax=docker/dockerfile:1.7

# Multi-stage build for the `roas` CLI binary. The release pipeline drives
# this with QEMU under the hood for linux/arm64, so the Dockerfile itself
# stays single-arch-shaped: each platform gets its own native build inside
# its own emulated container.

FROM rust:1-bookworm AS builder
WORKDIR /src
COPY . .
# `--locked` enforces the workspace `Cargo.lock` so the image build can't
# silently pick up a dep version that hasn't been tested yet.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    cargo build --release --package roas-cli --locked

# distroless/cc has glibc + libgcc_s — enough for a default-target Rust
# binary that doesn't pull in additional shared libraries. `nonroot` ships
# UID 65532 by default which is the more defensible posture for a CLI image.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /src/target/release/roas /usr/local/bin/roas
ENTRYPOINT ["/usr/local/bin/roas"]
