.PHONY: check lint test up down

check: lint test

lint:
	test -z "$$(gofmt -l .)"
	go vet ./...
	golangci-lint run ./...

test:
	go test -race ./...

up:
	docker compose up -d --build

down:
	docker compose down
