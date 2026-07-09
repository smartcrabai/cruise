# syntax=docker/dockerfile:1.25@sha256:0adf442eae370b6087e08edc7c50b552d80ddf261576f4ebd6421006b2461f12
FROM oven/bun:1-slim@sha256:d56a2534ffd262e92c12fd3249d3924d296d97086da773f821d7d0477435ea04

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
    useradd -m -s /bin/bash cruise && \
    mkdir -p /work /home/cruise/.cruise && \
    chown -R cruise:cruise /work /home/cruise/.cruise

COPY --chown=cruise:cruise --chmod=755 bin/${TARGETARCH}/cruise /usr/local/bin/cruise

WORKDIR /work
USER cruise
ENTRYPOINT ["cruise"]
