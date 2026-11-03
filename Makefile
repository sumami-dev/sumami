# Sumami — build helpers.
#
# The default build compiles DuckDB into the executables, so the release is a
# single self-contained binary. `make build-small` links the prebuilt
# libduckdb dynamically instead (smaller executable, one extra file to ship).

CARGO ?= cargo
CARGO_HTTP_TIMEOUT ?= 600
TARGET ?= target/release
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
build-small:
	CARGO_HTTP_TIMEOUT=$(CARGO_HTTP_TIMEOUT) $(CARGO) build --release --no-default-features
	@set -e; \
	dir=$$(ls -d target/duckdb-download/*/$(DUCKDB_VERSION) 2>/dev/null | head -1); \
	if [ -n "$$dir" ]; then \
		cp -f $$dir/libduckdb.* $(TARGET)/ 2>/dev/null || true; \
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
	$(CARGO) run -p sumami-service -- serve --db ./dev.duckdb --port 3000

## Build the release artifacts.
##
##   sumami-server          the server (standalone executable)
##   sumami-client          the analysis CLI (standalone executable)
##   sumami-<ver>-<os>-<arch>.tar.gz
##                          the two-in-one: the `sumami` binary + js/ + docs/
##
## The tarball deliberately does NOT contain all three binaries: each one carries its
## own copy of DuckDB, so shipping three of them triples the download for no gain.
## The two-in-one binary already contains both halves.
package: release
	@set -e; \
	rm -rf dist; mkdir -p dist; \
	cp $(TARGET)/sumami-server $(TARGET)/sumami-client $(TARGET)/sumami dist/; \
	mkdir -p dist/bundle; \
	cp $(TARGET)/sumami dist/bundle/; \
	cp -r js dist/bundle/js; cp -r docs dist/bundle/docs; \
	tar czf sumami-$(DUCKDB_VERSION)-$$(uname -s)-$$(uname -m).tar.gz -C dist/bundle .; \
	echo "wrote dist/sumami-server, dist/sumami-client and sumami-$(DUCKDB_VERSION)-$$(uname -s)-$$(uname -m).tar.gz"

## Remove the build artefacts.
clean:
	$(CARGO) clean
