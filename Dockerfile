FROM golang:1.27.2-alpine3.24 AS build

WORKDIR /src

COPY go.* ./
RUN go mod download

COPY . .
ARG VERSION=dev
RUN CGO_ENABLED=0 go build \
      -trimpath \
      -ldflags "-s -w -X main.version=${VERSION}" \
      -o /out/sei-monitor . \
    && mkdir -p /out/data

FROM alpine:3.24

# hadolint ignore=DL3018
RUN apk add --no-cache ca-certificates \
    && addgroup -S -g 65532 sei-monitor \
    && adduser -S -D -H -u 65532 -G sei-monitor sei-monitor

COPY --from=build /out/sei-monitor /usr/local/bin/sei-monitor
COPY --from=build --chown=65532:65532 /out/data /data

WORKDIR /data
VOLUME /data

USER 65532:65532

EXPOSE 8080

HEALTHCHECK --interval=1m --timeout=10s --start-period=5m --retries=3 \
    CMD ["sh", "-c", "address=\"${HTTP_ADDRESS-:8080}\"; [ -z \"$address\" ] || wget -q -O /dev/null \"http://127.0.0.1:${address##*:}/healthz\""]

ENTRYPOINT ["/usr/local/bin/sei-monitor"]
CMD ["serve"]
