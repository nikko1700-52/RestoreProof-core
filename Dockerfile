# A container image for the RestoreProof CLI.
#
# SECURITY NOTE — read before using this.
#
# RestoreProof drives a Docker daemon. Running it inside a container therefore
# means giving that container access to a daemon, usually by mounting the host's
# Docker socket:
#
#   docker run --rm \
#     -v /var/run/docker.sock:/var/run/docker.sock \
#     -v "$PWD:/drill:ro" -w /drill \
#     restoreproof:0.1.0 run --config restoreproof.yaml
#
# Mounting that socket grants the container full control of the host's Docker
# daemon, which is equivalent to root on the host. RestoreProof refuses exactly
# this mount inside a *recovery environment*, and the reasoning does not change
# just because it is the tool's own container.
#
# Prefer the native binary. Use this image when your CI already runs everything
# in containers and has made that trade-off deliberately — and never on a host
# whose daemon you do not fully control.
#
# Note also that the recovery environment's containers are started by the host
# daemon, so bind-mount paths are resolved on the *host*, not inside this image.

FROM rust:1.85-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p restoreproof-cli

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        docker.io \
        docker-compose-plugin \
        restic \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /src/target/release/restoreproof /usr/local/bin/restoreproof

# A non-root user by default. Add it to the group owning the mounted socket if
# you really do mount one.
RUN useradd --create-home --uid 10001 restoreproof
USER restoreproof
WORKDIR /drill

ENTRYPOINT ["restoreproof"]
CMD ["--help"]
