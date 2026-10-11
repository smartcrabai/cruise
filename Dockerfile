# syntax=docker/dockerfile:1.28@sha256:bb22d9815c728170f72750f4e5b0d672e06176142e1d602c7e66c050100b7e5b
FROM oven/bun:1-slim@sha256:14a223fa35747ec728bce300ef85888bc5f7958164d15bbba6a54b22b52a3288

ARG TARGETARCH=amd64
ARG GH_VERSION=2.72.0

SHELL ["/bin/bash", "-o", "pipefail", "-c"]
USER root
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates git curl && \
    arch=$(dpkg --print-architecture) && \
    curl -fsSL "https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_${arch}.tar.gz" \
        | tar -xz --strip-components=2 -C /usr/local/bin "gh_${GH_VERSION}_linux_${arch}/bin/gh" && \
    rm -rf /var/lib/apt/lists/* && \
    BUN_INSTALL=/usr/local bun install -g @anthropic-ai/claude-code@2.1.195 && \
    mkdir -p /work /home/bun/.cruise && \
    chown -R bun:bun /work /home/bun/.cruise

COPY --chown=bun:bun --chmod=755 bin/${TARGETARCH}/cruise /usr/local/bin/cruise

WORKDIR /work
USER bun
ENTRYPOINT ["cruise"]
