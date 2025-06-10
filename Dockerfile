# Build stage
FROM cgr.dev/chainguard/rust AS builder
WORKDIR /src
COPY . .
RUN cargo build --release --bin wash

# Final stage: distroless
FROM cgr.dev/chainguard/wolfi-base
RUN apk add --no-cache git
COPY --from=builder /src/target/release/wash /usr/local/bin/wash
ENTRYPOINT ["/usr/local/bin/wash"]