FROM kelnos/xfce-rust-build:latest

COPY Cargo.lock Cargo.toml Makefile .
RUN mkdir -p src && echo "fn main() {}" > src/main.rs
RUN cargo fetch
RUN cargo build -F debug --frozen

RUN rm -rf src
COPY resources ./resources
COPY src ./src
COPY .githooks ./.githooks
COPY rustfmt.toml deny.toml ./
RUN ./.githooks/pre-commit
RUN cargo test -F debug --frozen

ENTRYPOINT [ "/bin/sh" ]
