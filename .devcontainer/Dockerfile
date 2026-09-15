FROM mcr.microsoft.com/devcontainers/base:ubuntu-22.04

ENV DEBIAN_FRONTEND=noninteractive
ENV APPIMAGE_EXTRACT_AND_RUN=1
ENV PATH=/home/vscode/.cargo/bin:/usr/local/bin:${PATH}

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        desktop-file-utils \
        file \
        libayatana-appindicator3-dev \
        libdbus-1-dev \
        libfuse2 \
        libgtk-3-dev \
        librsvg2-dev \
        libssl-dev \
        libwebkit2gtk-4.1-dev \
        libxdo-dev \
        patchelf \
        pkg-config \
        rpm \
        wget \
        xdg-utils \
        xz-utils \
    && rm -rf /var/lib/apt/lists/*

# The first release target is Linux x86-64. Pinning the archive and checking
# its digest keeps local and CI builds on exactly the same Node patch release.
ARG NODE_VERSION=22.22.0
ARG NODE_LINUX_X64_SHA256=9aa8e9d2298ab68c600bd6fb86a6c13bce11a4eca1ba9b39d79fa021755d7c37
ARG PNPM_VERSION=12.4.1

RUN curl -fsSLO "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz" \
    && echo "${NODE_LINUX_X64_SHA256}  node-v${NODE_VERSION}-linux-x64.tar.xz" | sha256sum -c - \
    && tar -xJf "node-v${NODE_VERSION}-linux-x64.tar.xz" -C /usr/local --strip-components=1 \
    && rm "node-v${NODE_VERSION}-linux-x64.tar.xz" \
    && npm install --global "pnpm@${PNPM_VERSION}" \
    && pnpm --version

USER vscode

ARG RUST_VERSION=1.98.1

RUN curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}" \
    && rustup component add clippy rustfmt --toolchain "${RUST_VERSION}" \
    && rustc --version \
    && cargo --version

WORKDIR /workspaces/dispatch
