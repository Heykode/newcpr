# syntax=docker/dockerfile:1.7@sha256:a57df69d0ea827fb7266491f2813635de6f17269be881f696fbfdf2d83dda33e

# PostgreSQL 服务端镜像：备份客户端（pg_dump/pg_restore）从这里取，与 compose 同源、
# 版本严格一致。升级数据库版本时只需同步修改这里与 deploy/compose.yaml。
ARG POSTGRES_IMAGE=postgres:18-bookworm@sha256:bbf83594798e144bff365a9c191c5d779f181fb3135902064ab8ff11918020fc

FROM node:26.12.1-bookworm-slim@sha256:28147155d47dc31abe0a46e0d12b59b2fb3d0a2dcc5dc65fce76a074180b16fe AS frontend-builder

WORKDIR /app/frontend
COPY frontend/package.json frontend/pnpm-lock.yaml frontend/pnpm-workspace.yaml ./
RUN npm install --global pnpm@11.7.0 \
    && pnpm install --frozen-lockfile
COPY frontend ./
RUN pnpm build

FROM rust:1.99.1-bookworm@sha256:48b7f435b8fc6378ce43b44f15e3a47fd8919b2825f835c39bd4bd3da50af5b8 AS backend-toolchain

WORKDIR /app/backend

FROM backend-toolchain AS backend-builder
ARG TARGETARCH
ARG CPR_VERSION=
ARG CPR_GIT_SHA=unknown
ARG CPR_BUILD_TIME=unknown
ARG CPR_BUILD_TYPE=release
ENV CARGO_TARGET_DIR=/app/backend/target/${TARGETARCH}
ENV CPR_VERSION=${CPR_VERSION}
ENV CPR_GIT_SHA=${CPR_GIT_SHA}
ENV CPR_BUILD_TIME=${CPR_BUILD_TIME}
ENV CPR_BUILD_TYPE=${CPR_BUILD_TYPE}
COPY backend/Cargo.toml backend/Cargo.lock ./
COPY backend/apps ./apps
COPY backend/crates ./crates
COPY backend/migrations ./migrations
COPY release/version.yaml /app/release/version.yaml
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/backend/target \
    cargo build --release --locked --bin codex-proxy-rs \
    && cp "${CARGO_TARGET_DIR}/release/codex-proxy-rs" /tmp/codex-proxy-rs

# 备份客户端来源：与 compose 服务端同一固定镜像，版本一致。
FROM ${POSTGRES_IMAGE} AS pg-client

FROM debian:bookworm-slim@sha256:7ff6ba31527b9e336162fa061746c366c4b2032f4d2cff2078bea43f78efe3f8 AS runtime-base

ARG CPR_SECURITY_REFRESH=unknown
ENV TZ=Asia/Shanghai

# 运行时基础只从 Debian 官方源补 libpq（pg_dump 的动态依赖；ABI 兼容，无需引入第三方
# APT 源）。pg_dump/pg_restore 二进制由 pg-client stage COPY，构建期不依赖运行时安装。
# 更新基础镜像已有的系统包，使固定 digest 的镜像也能获取 Debian 安全更新。
RUN printf 'security refresh: %s\n' "${CPR_SECURITY_REFRESH}" >/dev/null \
    && apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get upgrade -y \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates curl tzdata libpq5 python3 python3-venv \
    && install -d /usr/lib/postgresql/18/bin \
    && ln -snf "/usr/share/zoneinfo/${TZ}" /etc/localtime \
    && printf '%s\n' "${TZ}" > /etc/timezone \
    && groupadd --system --gid 10001 cpr \
    && useradd --system --uid 10001 --gid cpr --home-dir /app --shell /usr/sbin/nologin cpr \
    && install -d -o cpr -g cpr \
        /app/bin /app/deploy /app/.runtime/data /app/.runtime/logs /app/web \
    && rm -rf /var/lib/apt/lists/*

COPY --from=pg-client /usr/lib/postgresql/18/bin/pg_dump /usr/lib/postgresql/18/bin/pg_dump
COPY --from=pg-client /usr/lib/postgresql/18/bin/pg_restore /usr/lib/postgresql/18/bin/pg_restore
RUN /usr/lib/postgresql/18/bin/pg_dump --version \
    && /usr/lib/postgresql/18/bin/pg_restore --version

WORKDIR /app
COPY deploy/relogin-requirements.txt /app/deploy/relogin-requirements.txt
# venv seeds setuptools; the login worker does not need this build-time package.
RUN python3 -m venv /opt/relogin \
    && /opt/relogin/bin/pip install --no-cache-dir -r /app/deploy/relogin-requirements.txt \
    && /opt/relogin/bin/pip uninstall -y setuptools \
    && /opt/relogin/bin/pip check \
    && /opt/relogin/bin/python3 -I -c "\
import importlib.util; \
from curl_cffi import requests; \
import pyotp; \
assert importlib.util.find_spec('setuptools') is None; \
assert pyotp.TOTP('GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ').at(59) == '287082'; \
requests.Session(impersonate='firefox133').close()"
ENV CPR_DEPLOYMENT_MODE=docker
ENV PATH="/opt/relogin/bin:/app/bin:/usr/lib/postgresql/18/bin:${PATH}"

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1

CMD ["/app/bin/codex-proxy-rs"]

FROM runtime-base AS runtime

COPY --from=backend-builder --chown=cpr:cpr /tmp/codex-proxy-rs /app/bin/codex-proxy-rs
COPY --from=frontend-builder --chown=cpr:cpr /app/frontend/dist ./web/dist
USER cpr

FROM runtime-base AS runtime-ci

ARG CPR_PREBUILT_DIR=dist/docker
COPY --chown=cpr:cpr ${CPR_PREBUILT_DIR}/linux-amd64/codex-proxy-rs /app/bin/codex-proxy-rs
COPY --from=frontend-builder --chown=cpr:cpr /app/frontend/dist ./web/dist
USER cpr

FROM runtime-base AS runtime-prebuilt

ARG TARGETARCH
ARG CPR_PREBUILT_DIR=dist/docker

COPY --chown=cpr:cpr ${CPR_PREBUILT_DIR}/linux-${TARGETARCH}/codex-proxy-rs /app/bin/codex-proxy-rs
COPY --chown=cpr:cpr ${CPR_PREBUILT_DIR}/web/dist ./web/dist
RUN chmod +x /app/bin/codex-proxy-rs
USER cpr
