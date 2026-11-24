WASM_TARGET := wasm32-unknown-unknown
HOST_TARGET := $(shell rustc -vV | sed -n 's/host: //p')
OUT  := target/$(WASM_TARGET)/release/deckwatch_plugin_otel.wasm
DIST := dist/plugin.wasm

.PHONY: build test lint fmt clean

# dist/plugin.wasm plus dist/plugin.wasm.sha256 (the value to pin in
# deckwatch's plugin `sha256` setting).
build:
	cargo build --release --target $(WASM_TARGET)
	mkdir -p dist
	cp $(OUT) $(DIST)
	cd dist && shasum -a 256 plugin.wasm > plugin.wasm.sha256
	@echo "artifact: $(DIST) ($$(wc -c < $(DIST)) bytes)"
	@echo "sha256:   $$(cut -d' ' -f1 dist/plugin.wasm.sha256)"

# .cargo/config.toml defaults to wasm32, so tests name the host target.
test:
	cargo test --target $(HOST_TARGET)

lint:
	cargo fmt --check
	cargo clippy --target $(HOST_TARGET) --all-targets -- -D warnings
	cargo clippy --target $(WASM_TARGET) -- -D warnings

fmt:
	cargo fmt

clean:
	cargo clean
	rm -f $(DIST) $(DIST).sha256
