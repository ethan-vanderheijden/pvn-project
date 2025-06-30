FROM rust:latest AS builder

RUN mkdir -p /pvn
WORKDIR /pvn
COPY Cargo.toml Cargo.toml
COPY Cargo.lock Cargo.lock
COPY tls_validator tls_validator
COPY bittorrent_offload bittorrent_offload
RUN cargo install --path tls_validator
RUN cargo install --path bittorrent_offload

FROM ubuntu:latest AS tls_validator
COPY --from=builder /usr/local/cargo/bin/tlsv /usr/local/bin/tlsv
ENTRYPOINT ["tlsv"]

FROM ubuntu:latest AS bittorrent_offload
COPY --from=builder /usr/local/cargo/bin/bittorrent_offload /usr/local/bin/bittorrent_offload
ENTRYPOINT ["bittorrent_offload"]
