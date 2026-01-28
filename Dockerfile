FROM rust:1.88.0-slim-trixie

WORKDIR /src/xfwl4

RUN DEBIAN_FRONTEND=noninteractive apt-get update && \
    DEBIAN_FRONTEND=noninteractive apt-get install -y \
        build-essential \
        libdisplay-info-dev \
        libdrm-dev \
        libgbm-dev \
        libgtk-3-dev \
        libinput-dev \
        libpixman-1-dev \
        libseat-dev \
        libudev-dev \
        libxfconf-0-dev \
        libxkbcommon-dev \
        pkg-config

COPY Cargo.lock Cargo.toml Makefile .
RUN mkdir -p src && echo "fn main() {}" > src/main.rs
RUN cargo fetch
RUN cargo build -F debug --frozen

RUN rm -rf src
COPY resources ./resources
COPY src ./src
RUN cargo build -F debug --frozen

ENTRYPOINT [ "/bin/sh" ]
