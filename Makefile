BIN = target/x86_64-unknown-linux-musl/release/Rex
SSTRIP := $(shell command -v sstrip 2>/dev/null)

all:
	cargo clean
	cargo build --release
	printf '\x52\x45\x58' | dd of=$(BIN) bs=1 seek=8 conv=notrunc >/dev/null 2>&1
	xxd -l 16 -g 1 $(BIN)

	@if [ -n "$(SSTRIP)" ]; then \
		$(SSTRIP) $(BIN); \
	fi
