# syntax=docker/dockerfile:1.29@sha256:572cebbc2b15e6f84c077742952674889cb63cd5e9b80abdffcbe587b6f19183
FROM oven/bun:1-slim@sha256:b780593f3e10918cbdaec3d5a08d52224b9198069a65eb47812085c2ec007643

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
COPY --chown=bun:bun --chmod=755 bin/${TARGETARCH}/cruise-runtime /usr/local/bin/cruise-runtime

WORKDIR /work
USER bun
EXPOSE 8484
ENTRYPOINT ["cruise"]
CMD ["daemon","serve","--host","0.0.0.0","--port","8484"]
