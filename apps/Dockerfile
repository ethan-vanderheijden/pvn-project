FROM rust:latest AS builder

RUN mkdir -p /pvn
WORKDIR /pvn
COPY . .
RUN cargo install --path tls_validator
RUN cargo install --path bittorrent_offload
RUN cargo install --path rdr_parent_cache

FROM ubuntu:latest AS tls_validator
COPY --from=builder /usr/local/cargo/bin/tlsv /usr/local/bin/tlsv
ENTRYPOINT ["tlsv"]

FROM ubuntu:latest AS bittorrent_offload
COPY --from=builder /usr/local/cargo/bin/bittorrent_offload /usr/local/bin/bittorrent_offload
ENTRYPOINT ["bittorrent_offload"]

FROM zenika/alpine-chrome:latest AS rdr_parent_cache
USER root
RUN apt-get update && apt-get install -y libssl-dev && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/cargo/bin/rdr_parent_cache /usr/local/bin/rdr_parent_cache
RUN useradd -ms /bin/bash pvn
USER pvn
ENTRYPOINT ["rdr_parent_cache"]
