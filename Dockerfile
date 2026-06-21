# Build the Fleety server.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked --bin fleety-server

# Slim runtime with the tools the agent shells out to (git, ssh, TLS roots).
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates git openssh-client \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/fleety-server /usr/local/bin/fleety-server
# Listen on all interfaces inside the container; persist state under /data.
ENV FLEETY_ADDR=0.0.0.0:8787 \
    FLEETY_AGENT_HOME=/data/agent \
    FLEETY_WORKSPACE=/workspace
EXPOSE 8787
VOLUME ["/data", "/workspace"]
CMD ["fleety-server"]
