.PHONY: check lint test openapi up down

check: lint test

lint:
	test -z "$$(gofmt -l .)"
	go vet ./...
	golangci-lint run ./...

test:
	go test -race ./...

openapi:
	go run . openapi > docs/openapi.json

up:
	docker compose up -d --build

down:
	docker compose down
