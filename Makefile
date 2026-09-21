# maskrun
PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin

.PHONY: help build test install uninstall guard lint

help:
	@echo "make build      cargo build --release"
	@echo "make test       cargo test (single-threaded: keyring backends share state)"
	@echo "make install    copy the release binary to $(BINDIR)"
	@echo "make uninstall  remove it again"
	@echo "make guard      register the agent guard hook"
	@echo "make lint       cargo fmt --check + clippy -D warnings"

build:
	cargo build --release

# Single-threaded: keyring backend tests hit the same Secret Service /
# Keychain / Credential Manager and would otherwise race each other.
test:
	cargo test -- --test-threads=1

install: build
	@mkdir -p $(BINDIR)
	install -m 755 target/release/maskrun $(BINDIR)/maskrun
	@echo "installed -> $(BINDIR)/maskrun"
	@command -v maskrun >/dev/null 2>&1 || \
		echo "note: $(BINDIR) is not on your PATH"

uninstall:
	rm -f $(BINDIR)/maskrun
	@echo "removed $(BINDIR)/maskrun"

guard: build
	target/release/maskrun install-guard

lint:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
