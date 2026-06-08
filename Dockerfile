FROM rust:1-bookworm AS builder

# Install zig and cargo-zigbuild
RUN apt-get update && apt-get install -y --no-install-recommends \
    xz-utils \
    && rm -rf /var/lib/apt/lists/*

ARG ZIG_VERSION=0.15.2
RUN case "$(uname -m)" in \
      x86_64)  ZIG_ARCH=x86_64 ;; \
      aarch64) ZIG_ARCH=aarch64 ;; \
      *) echo "Unsupported arch: $(uname -m)" && exit 1 ;; \
    esac && \
    curl --retry 5 --retry-all-errors --retry-delay 5 --connect-timeout 30 --max-time 600 -fsSL \
      -o /tmp/zig.tar.xz \
      "https://ziglang.org/download/${ZIG_VERSION}/zig-${ZIG_ARCH}-linux-${ZIG_VERSION}.tar.xz" && \
    tar -xJf /tmp/zig.tar.xz -C /usr/local && \
    rm -f /tmp/zig.tar.xz && \
    ln -s /usr/local/zig-${ZIG_ARCH}-linux-${ZIG_VERSION}/zig /usr/local/bin/zig

RUN cargo install cargo-zigbuild && \
    rustup target add x86_64-unknown-linux-musl

WORKDIR /build

# Clone C++ engine source
ARG DECK_CPP_REPO=https://github.com/Team-Haruki/sekai-deck-recommend-cpp.git
ARG DECK_CPP_BRANCH=master
ARG DECK_CPP_REF=24a26394f7c9d82c653dc00d2bc1c3a9efc7423f
RUN git clone --branch "${DECK_CPP_BRANCH}" --single-branch "${DECK_CPP_REPO}" _cpp_src && \
    cd _cpp_src && \
    git checkout "${DECK_CPP_REF}" && \
    git submodule update --init --recursive

# Copy project files
COPY Cargo.toml Cargo.lock build.rs build.zig cpp_sources.txt ./
COPY .cargo .cargo
COPY src/ src/
COPY cpp_bridge/ cpp_bridge/

# Build static binary
RUN cargo zigbuild --release --target x86_64-unknown-linux-musl && \
    cp target/x86_64-unknown-linux-musl/release/deck-service /deck-service && \
    strip /deck-service

# Copy static data needed at runtime
RUN cp -r _cpp_src/data /data

# --- Final minimal image ---
FROM scratch

COPY --from=builder /deck-service /deck-service
COPY --from=builder /data /data

ENV DECK_DATA_DIR=/data
ENV BIND_ADDR=0.0.0.0:3000

EXPOSE 3000

ENTRYPOINT ["/deck-service"]
