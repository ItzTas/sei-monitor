.PHONY: check lint test openapi local up down

check: lint test

lint:
	test -z "$$(gofmt -l .)"
	go vet ./...
	golangci-lint run ./...

test:
	go test -race ./...

openapi:
	go run . openapi > docs/openapi.json

local:
	./scripts/local.sh $(CMD)

up:
	docker compose up -d --build

down:
	docker compose down
