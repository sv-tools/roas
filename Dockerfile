# syntax=docker/dockerfile:1.7
#
# Release-pipeline Dockerfile. Expects prebuilt linux binaries staged in the
# build context at `./linux/amd64/roas` and `./linux/arm64/roas`. Buildx
# substitutes TARGETOS/TARGETARCH per platform when running with
# `--platform linux/amd64,linux/arm64`, so a single Dockerfile + COPY yields
# the right multi-arch image without QEMU rebuilds of the rust toolchain.
#
# To build locally (single arch matching the host):
#   cargo build --release --package roas-cli
#   mkdir -p ctx/linux/$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')
#   cp target/release/roas ctx/linux/$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')/
#   docker build -t roas:dev ctx

FROM gcr.io/distroless/cc-debian13:nonroot
ARG TARGETOS
ARG TARGETARCH
COPY ${TARGETOS}/${TARGETARCH}/roas /usr/local/bin/roas
ENTRYPOINT ["/usr/local/bin/roas"]
