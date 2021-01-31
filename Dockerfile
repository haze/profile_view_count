# Build Stage
FROM rust:1.49.0 AS builder
WORKDIR /usr/src/
RUN rustup target add x86_64-unknown-linux-musl

RUN USER=root cargo new profile_view_count
WORKDIR /usr/src/profile_view_count
COPY Cargo.toml Cargo.lock ./
RUN cargo build --release

COPY src ./src
COPY view_count_template.svg /view_count_template.svg
COPY colors.txt /colors.txt
RUN cargo install --target x86_64-unknown-linux-musl --path .

# Bundle Stage
FROM scratch
COPY --from=builder /usr/local/cargo/bin/profile_view_count .
COPY --from=builder /view_count_template.svg .
COPY --from=builder /colors.txt .
USER 1000
CMD ["./profile_view_count"]
