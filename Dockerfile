# syntax=docker/dockerfile:1.25
FROM oven/bun:1-slim

ARG TARGETARCH=amd64
ARG GH_VERSION=2.72.0

USER root
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
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
