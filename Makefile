all: proto

# protobuf
.PHONY: proto
proto: proto-generate

.PHONY: proto
proto-generate:
	docker run --volume "$(PWD):/workspace" --workdir /workspace bufbuild/buf generate
	
.PHONY: proto-format
proto-format:
	docker run --volume "$(PWD):/workspace" --workdir /workspace bufbuild/buf format -w

.PHONY: proto-lint
proto-lint:
	docker run --volume "$(PWD):/workspace" --workdir /workspace bufbuild/buf lint
