FROM node:24-bookworm AS web
WORKDIR /src/xbluenode-web
COPY xbluenode-web/package.json xbluenode-web/package-lock.json ./
RUN npm ci
COPY xbluenode-web/ ./
RUN npm run build

FROM rust:1.100-bookworm AS core
WORKDIR /src
COPY axpoint-core/ axpoint-core/
COPY deploy/migrations/ deploy/migrations/
COPY --from=web /src/xbluenode-web/src/ xbluenode-web/src/
RUN mkdir -p /out/nonroot-home/.duckdb/extensions \
 && cd axpoint-core && cargo build --release -p axpoint-server

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=core /src/axpoint-core/target/release/axpoint-server /app/axpoint-server
COPY --from=web /src/xbluenode-web/dist /app/web
COPY --from=core --chown=65532:65532 /out/nonroot-home/ /home/nonroot/
ENV HOME=/home/nonroot WEB_DIST_DIR=/app/web AXPOINT_BIND_ADDR=0.0.0.0:9995 AXPOINT_DATA_ROOT=/var/lib/axpoint
EXPOSE 9995
USER nonroot
HEALTHCHECK --interval=10s --timeout=3s --retries=6 CMD ["/app/axpoint-server", "healthcheck"]
ENTRYPOINT ["/app/axpoint-server"]
