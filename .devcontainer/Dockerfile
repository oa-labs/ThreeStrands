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

# The release build targets Linux x86-64; the development container
# (.devcontainer/dev) builds natively, so it may be arm64 on Apple Silicon.
# Pinning each archive and checking its digest keeps local and CI builds on
# exactly the same Node patch release.
ARG TARGETARCH
ARG NODE_VERSION=22.22.0
ARG NODE_LINUX_X64_SHA256=9aa8e9d2298ab68c600bd6fb86a6c13bce11a4eca1ba9b39d79fa021755d7c37
ARG NODE_LINUX_ARM64_SHA256=1bf1eb9ee63ffc4e5d324c0b9b62cf4a289f44332dfef9607cea1a0d9596ba6f
ARG PNPM_VERSION=12.16.1

RUN case "${TARGETARCH:-amd64}" in \
      amd64) node_arch=x64; node_sha256="${NODE_LINUX_X64_SHA256}" ;; \
      arm64) node_arch=arm64; node_sha256="${NODE_LINUX_ARM64_SHA256}" ;; \
      *) echo "unsupported architecture: ${TARGETARCH}" >&2; exit 1 ;; \
    esac \
    && node_archive="node-v${NODE_VERSION}-linux-${node_arch}.tar.xz" \
    && curl -fsSLO "https://nodejs.org/dist/v${NODE_VERSION}/${node_archive}" \
    && echo "${node_sha256}  ${node_archive}" | sha256sum -c - \
    && tar -xJf "${node_archive}" -C /usr/local --strip-components=1 \
    && rm "${node_archive}" \
    && npm install --global "pnpm@${PNPM_VERSION}" \
    && pnpm --version

USER vscode

ARG RUST_VERSION=1.98.1

RUN curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}" \
    && rustup component add clippy rustfmt --toolchain "${RUST_VERSION}" \
    && rustc --version \
    && cargo --version

# Dependency review tools for the development container. The release build
# leaves this off so its image stays small.
ARG INSTALL_SUPPLY_CHAIN_TOOLS=0
ARG CARGO_VET_VERSION=0.10.3
ARG CARGO_DENY_VERSION=0.20.2

RUN if [ "${INSTALL_SUPPLY_CHAIN_TOOLS}" = "1" ]; then \
      cargo install --locked "cargo-vet@${CARGO_VET_VERSION}" "cargo-deny@${CARGO_DENY_VERSION}" \
      && rm -rf /home/vscode/.cargo/registry \
      && cargo vet --version \
      && cargo deny --version; \
    fi

WORKDIR /workspaces/threestrands
