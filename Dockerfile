# Build the Fleety server.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked --bin fleety-server

# Build the fleety-insyra data-analysis sidecar (Go) at the pinned Insyra version
# (go.mod). Release CI bumps that to @latest; Docker builds stay reproducible.
FROM golang:1-bookworm AS gobuild
WORKDIR /sidecar
COPY sidecars/fleety-insyra/ ./
RUN go build -trimpath -o /out/fleety-insyra .

# Slim runtime with the tools the agent shells out to (git, ssh, TLS roots,
# Python for the built-in `ddgs` web-search MCP, chromium for the browser/CDP
# tools).
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates git openssh-client \
      python3 python3-pip pipx \
      chromium \
      libgomp1 \
 && rm -rf /var/lib/apt/lists/* \
 # Built-in `ddgs` MCP (search_text / search_images / search_news / search_videos
 # / search_books / extract_content) — installed into a system-managed pipx venv
 # under /opt/pipx, with the `ddgs` launcher symlinked into /usr/local/bin so it is
 # on PATH for every user (including the non-root runtime user added below).
 && PIPX_HOME=/opt/pipx PIPX_BIN_DIR=/usr/local/bin pipx install "ddgs[mcp]"
COPY --from=build /src/target/release/fleety-server /usr/local/bin/fleety-server
COPY --from=gobuild /out/fleety-insyra /usr/local/bin/fleety-insyra
# Run as a dedicated non-root user (fixed uid) so files the server writes into the
# /data and /workspace volumes are owned by that uid, not root on the host. Create
# the writable state dirs and hand them (and the user's home) to fleety *before*
# the VOLUME lines, so anonymous volumes inherit the ownership. Bind mounts keep
# the host directory's owner — mount with a matching uid if you need a specific one.
RUN useradd --uid 10001 --create-home --home-dir /home/fleety --shell /usr/sbin/nologin fleety \
 && mkdir -p /data /workspace \
 && chown -R fleety:fleety /data /workspace /home/fleety
# Listen on all interfaces inside the container; persist state under /data. The
# browser tools find `chromium` on PATH; managed Chrome downloads (if ever needed)
# persist on the /data volume. HOME and the managed runtimes/models directories
# also sit on fleety-writable paths so nothing writes to a root-owned location.
ENV HOME=/home/fleety \
    FLEETY_ADDR=0.0.0.0:8787 \
    FLEETY_AGENT_HOME=/data/agent \
    FLEETY_WORKSPACE=/workspace \
    FLEETY_CHROME_DIR=/data/chrome \
    FLEETY_MODELS_DIR=/data/models \
    FLEETY_RUNTIMES_DIR=/data/runtimes
EXPOSE 8787
VOLUME ["/data", "/workspace"]
USER fleety
CMD ["fleety-server"]
