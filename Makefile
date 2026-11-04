# Sumimi — build helpers.
#
# The default build compiles DuckDB into the executables, so the release is a
# single self-contained binary. `make build-small` links the prebuilt
# libduckdb dynamically instead (smaller executable, one extra file to ship).

CARGO ?= cargo
CARGO_HTTP_TIMEOUT ?= 600
TARGET ?= target/release
# The version of the release: the crate version, not DuckDB's.
VERSION ?= 0.3.0
# The DuckDB release the dynamic build downloads.
DUCKDB_VERSION ?= v2.0.0

.PHONY: all build release test lint fmt run clean package build-small

all: build

## Build (debug).
build:
	$(CARGO) build

## Build (release): static DuckDB, self-contained binaries.
release:
	CARGO_HTTP_TIMEOUT=$(CARGO_HTTP_TIMEOUT) $(CARGO) build --release

## Build (release) with a dynamically linked DuckDB: smaller, needs libduckdb.
##
## The binary looks for the library next to itself (`@loader_path` / `$ORIGIN`),
## so the release is still a directory you can copy as a whole.
##
## The downloaded library is a universal binary (x86_64 + arm64) — the slice for
## this machine is kept, which halves it.
build-small:
	CARGO_HTTP_TIMEOUT=$(CARGO_HTTP_TIMEOUT) $(CARGO) build --release --no-default-features --features duckdb-dynamic
	@set -e; \
	dir=$$(ls -d target/duckdb-download/*/$(DUCKDB_VERSION) 2>/dev/null | head -1); \
	if [ -n "$$dir" ]; then \
		for lib in $$dir/libduckdb.*; do \
			cp -f $$lib $(TARGET)/; \
			if command -v lipo >/dev/null 2>&1; then \
				lipo $$lib -thin $$(uname -m) -output $(TARGET)/$$(basename $$lib) 2>/dev/null || true; \
			fi; \
		done; \
		echo "libduckdb copied from $$dir"; \
	fi

## Run the tests.
test:
	$(CARGO) test --workspace

## Run clippy with warnings as errors.
lint:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

## Format the code.
fmt:
	$(CARGO) fmt --all

## Run the server locally.
run:
	$(CARGO) run -p sumimi-service -- serve --db ./dev.duckdb --port 3000

## Build the release artefact.
##
##   sumimi-v<ver>-<os>-<arch>.tar.gz
##       the `sumimi` binary (server + client) + js/ + docs/
##
## One binary: the server and the client share the schema, the metric definitions and
## the query code, so there is only one copy of DuckDB in it.
package: release
	@set -e; \
	rm -rf dist; mkdir -p dist/bundle; \
	cp $(TARGET)/sumimi dist/bundle/; \
	ln -sf sumimi dist/bundle/sumimi-server; \
	ln -sf sumimi dist/bundle/sumimi-client; \
	cp -r js dist/bundle/js; cp -r service/docs dist/bundle/docs; \
	tar czf sumimi-v$(VERSION)-$$(uname -s)-$$(uname -m).tar.gz -C dist/bundle .; \
	echo "wrote sumimi-v$(VERSION)-$$(uname -s)-$$(uname -m).tar.gz"

## Remove the build artefacts.
clean:
	$(CARGO) clean
