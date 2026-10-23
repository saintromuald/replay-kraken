# Build
FROM rust:1-slim AS build
WORKDIR /app
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release

# Runtime
FROM debian:bookworm-slim
WORKDIR /app
COPY --from=build /app/target/release/replay-kraken /app/replay-kraken
COPY static ./static
VOLUME /app/data
EXPOSE 8787
CMD ["/app/replay-kraken"]
