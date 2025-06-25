FROM rust:latest AS builder

RUN mkdir -p /pvn
WORKDIR /pvn
COPY Cargo.toml Cargo.toml
COPY Cargo.lock Cargo.lock
COPY tls_validator tls_validator
RUN cargo install --path tls_validator

FROM ubuntu:latest AS tls_validator
COPY --from=builder /usr/local/cargo/bin/tlsv /usr/local/bin/tlsv
ENTRYPOINT ["tlsv"]
