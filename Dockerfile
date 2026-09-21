# syntax=docker/dockerfile:1

# Build a fully static musl binary. Alpine already ships musl-gcc, so no extra
# packages are needed and the result has no glibc dependency. Building natively
# per platform (rather than cross-compiling a fixed target) keeps this correct
# under `buildx --platform linux/amd64,linux/arm64`.
FROM rust:alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /build
# rust-toolchain.toml is deliberately not copied: it requests rustfmt, clippy
# and rust-src for local development, and honouring it here would make rustup
# download those components on every image build.
COPY Cargo.toml Cargo.lock ./
# Prime the dependency cache so later source edits do not rebuild every crate.
RUN mkdir src && printf 'fn main() {}\n' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY src ./src
RUN touch src/main.rs && cargo build --release --locked

# The final image is empty: no shell, no libc, no package manager, no CA store.
# Every runtime dependency is compiled into the binary:
#   - TLS roots via webpki-roots
#   - the IANA timezone database via the bundled jiff tzdb
FROM scratch

COPY --from=builder /build/target/release/hkmu-ole-attendance /hkmu-ole-attendance

ENTRYPOINT ["/hkmu-ole-attendance"]
