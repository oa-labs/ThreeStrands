FROM rust:1.85-bookworm AS build
WORKDIR /app
COPY crates/sync-protocol crates/sync-protocol
COPY services/sync-server services/sync-server
RUN cargo build --release --manifest-path services/sync-server/Cargo.toml

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/services/sync-server/target/release/threestrands-sync-server /usr/local/bin/threestrands-sync-server
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["threestrands-sync-server"]

