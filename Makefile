.PHONY: check lint test generate openapi dev local up down

check: lint test

lint:
	test -z "$$(gofmt -l .)"
	go vet ./...
	golangci-lint run ./...

test:
	go test -race ./...

generate:
	go tool sqlc generate

openapi:
	go run . openapi > docs/openapi.json

dev:
	go tool air

local:
	go run . $(CMD)

up:
	docker compose up -d --build

down:
	docker compose down
