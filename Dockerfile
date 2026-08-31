# ---- build ----
FROM rust:1-slim-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release -p kikimimi-cloud

# ---- runtime ----
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/kikimimi-cloud /usr/local/bin/kikimimi-cloud
ENV BIND_ADDR=0.0.0.0:8787
EXPOSE 8787
USER nobody
CMD ["kikimimi-cloud"]
