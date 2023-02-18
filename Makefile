.DEFAULT_GOAL:=help

.PHONY: test test-integration test-all clean help

##@ Testing

test: ## Run unit test suite
	cargo nextest run --no-fail-fast -p wash-lib
	cargo nextest run --no-fail-fast --bin wash

test-integration: ##Run integration test suite
	docker compose -f ./tools/docker-compose.yml up --detach
	cargo nextest run --profile integration -E 'kind(test)'
	docker compose -f ./tools/docker-compose.yml down

rust-check:
	cargo fmt --all --check
	cargo clippy --all-features --all-targets --workspace

# run all tests
test-all:
	$(MAKE) test
	$(MAKE) test-integration
	$(MAKE) rust-check

##@ Helpers

clean:
	cargo clean
	wash drain all

# Checks dependencies necessary for developing wash.
check-deps:
	python3 tools/check_deps.py

help:  ## Display this help
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_\-.*]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)