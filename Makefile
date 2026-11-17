.PHONY: check test up down

TOOLCHAIN := $(shell sed -n 's/^channel = "\(.*\)"$$/\1/p' rust-toolchain.toml)
CARGO := $(if $(shell command -v rustup),rustup run $(TOOLCHAIN) cargo,cargo)

check:
	$(CARGO) fmt --check
	$(CARGO) clippy --all-targets --all-features -- -D warnings
	$(CARGO) test

test:
	$(CARGO) test

up:
	docker compose up -d

down:
	docker compose down
