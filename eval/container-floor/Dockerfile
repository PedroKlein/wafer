ARG BUILD_IMAGE

FROM ${BUILD_IMAGE} AS build
WORKDIR /worker
COPY worker/ ./
RUN RUSTFLAGS="-C target-feature=+crt-static" cargo build --locked --release

FROM scratch
COPY --from=build /worker/target/release/container-floor-worker /worker
ENTRYPOINT ["/worker"]
