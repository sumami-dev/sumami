# Sumami — build helpers.
#
# DuckDB is linked dynamically so the executables stay small. The library is
# downloaded at build time and copied next to the binaries so it is found at
# runtime via the RPATH ($ORIGIN on Linux, @loader_path on macOS).

CARGO ?= cargo
CARGO_HTTP_TIMEOUT ?= 600
TARGET ?= target/release
DUCKDB_VERSION ?= v2.0.0

.PHONY: all build release test lint fmt run clean package libduckdb

all: build

## Build (debug).
build:
	$(CARGO) build

## Build (release) and put libduckdb next to the binaries.
release: libduckdb
	CARGO_HTTP_TIMEOUT=$(CARGO_HTTP_TIMEOUT) $(CARGO) build --release

## Copy the downloaded DuckDB library next to the built binaries.
libduckdb:
	@set -e; \
	dir=$$(ls -d target/duckdb-download/*/$(DUCKDB_VERSION) 2>/dev/null | head -1); \
	if [ -n "$$dir" ]; then \
		cp -f $$dir/libduckdb.* target/debug/ 2>/dev/null || true; \
		cp -f $$dir/libduckdb.* $(TARGET)/ 2>/dev/null || true; \
		echo "libduckdb copied from $$dir"; \
	else \
		echo "libduckdb not found in target/duckdb-download — run 'cargo build' first"; \
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
run: libduckdb
	$(CARGO) run -p sumami-service -- serve --db ./dev.duckdb --port 3000

## Build a release archive: binaries + libduckdb + the frontend files.
package: release
	@set -e; \
	rm -rf dist; mkdir -p dist; \
	cp $(TARGET)/sumami-server $(TARGET)/sumami dist/; \
	dir=$$(ls -d target/duckdb-download/*/$(DUCKDB_VERSION) 2>/dev/null | head -1); \
	[ -n "$$dir" ] && cp -f $$dir/libduckdb.* dist/ || true; \
	cp -r js dist/js; cp -r docs dist/docs; \
	tar czf sumami-$(DUCKDB_VERSION)-$$(uname -s)-$$(uname -m).tar.gz -C dist .; \
	echo "wrote sumami-$(DUCKDB_VERSION)-$$(uname -s)-$$(uname -m).tar.gz"

## Remove the build artefacts.
clean:
	$(CARGO) clean
