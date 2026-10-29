PREFIX ?= /usr
DESTDIR ?=
CARGO ?= cargo
# Where the built programs are: target/aarch64-unknown-linux-gnu/release
# after build-aarch64.sh.
BIN ?= target/release

.PHONY: all check install
all:
	$(CARGO) build --release --locked
check:
	$(CARGO) fmt --check
	$(CARGO) clippy --locked --all-targets -- -D warnings
	$(CARGO) test --locked
install:
	install -Dm755 $(BIN)/ledgerd $(DESTDIR)$(PREFIX)/bin/ledgerd
	install -Dm755 $(BIN)/ledger $(DESTDIR)$(PREFIX)/bin/ledger
	install -Dm644 packaging/ledgerd.service $(DESTDIR)$(PREFIX)/lib/systemd/system/ledgerd.service
	sed -i 's|/usr/bin/ledgerd|$(PREFIX)/bin/ledgerd|g' $(DESTDIR)$(PREFIX)/lib/systemd/system/ledgerd.service
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/ledger/LICENSE
