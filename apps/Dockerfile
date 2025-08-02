FROM rust:latest AS builder

RUN mkdir -p /pvn
WORKDIR /pvn
COPY . .
RUN cargo install --path tls_validator
RUN cargo install --path bittorrent_offload
RUN cargo install --path rdr_parent_cache
RUN cargo install --path video_transcoder

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

FROM rust:latest AS video_transcoder
COPY --from=builder /usr/local/cargo/bin/video_transcoder /usr/local/bin/video_transcoder
RUN mkdir /pvn
WORKDIR /pvn

RUN apt-get update && apt-get install -y \
        libgstreamer1.0-dev \
        libgstreamer-plugins-base1.0-dev \
        libgstreamer-plugins-bad1.0-dev \
        gstreamer1.0-plugins-base \
        gstreamer1.0-plugins-good \
        gstreamer1.0-plugins-bad \
        gstreamer1.0-plugins-ugly \
        gstreamer1.0-libav \
        gstreamer1.0-tools \
        git \
        gcc \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install cargo-c
RUN git clone https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs.git
WORKDIR gst-plugins-rs
RUN cargo cbuild -p gst-plugin-fmp4 --prefix=/usr
RUN cargo cinstall -p gst-plugin-fmp4 --prefix=/usr
WORKDIR /pvn

COPY video_transcoder/vp9_moov.mp4 vp9_moov.mp4
COPY video_transcoder/gst_vp9_transcode.c gst_vp9_transcode.c
RUN gcc -o gst_vp9_transcode gst_vp9_transcode.c $(pkg-config --cflags --libs gstreamer-1.0)
ENTRYPOINT ["video_transcoder"]
