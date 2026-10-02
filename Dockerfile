# syntax=docker/dockerfile:1.27@sha256:4edf897a3ffa55b89f906fc8cc78afdb3f1834cc9c7083565e611a8a7d5fe99e
FROM oven/bun:1-slim@sha256:cb3bbbb08e13a4a2ff400f24c7a2a1d5efa83f6ef8544d52d95a519631e2fc61

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
