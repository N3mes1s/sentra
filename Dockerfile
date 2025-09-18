## Multi-stage build for Sentra
FROM rust:1.82 as builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY examples ./examples
COPY benches ./benches
RUN cargo build --release --bin sentra

FROM gcr.io/distroless/cc-debian12:nonroot
WORKDIR /app
COPY --from=builder /app/target/release/sentra /app/sentra
USER nonroot
EXPOSE 8080
ENV RUST_LOG=info
ENTRYPOINT ["/app/sentra"]