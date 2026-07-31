.PHONY: check test build install web

check:
	cargo check --workspace

test:
	cargo test --workspace

build:
	cargo build --release --workspace

install: build
	mkdir -p "$(HOME)/.local/bin"
	install -m755 target/release/couchlink-signaling "$(HOME)/.local/bin/couchlink-signaling"
	install -m755 target/release/couchlink-host "$(HOME)/.local/bin/couchlink-host"
	install -m755 target/release/couchlink-client "$(HOME)/.local/bin/couchlink-client"
