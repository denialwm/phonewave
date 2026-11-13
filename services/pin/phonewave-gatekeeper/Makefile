PREFIX ?= /usr
DESTDIR ?=
CARGO ?= cargo
# The Rust target of a cross build, such as aarch64-unknown-linux-gnu.
TARGET ?=
BIN = target/$(if $(TARGET),$(TARGET)/)release

.PHONY: all check install
all:
	$(CARGO) build --release --locked $(if $(TARGET),--target $(TARGET))
check:
	$(CARGO) fmt --check
	$(CARGO) clippy --locked --all-targets -- -D warnings
	$(CARGO) test --locked
install:
	install -Dm755 $(BIN)/phonewave-gatekeeper $(DESTDIR)$(PREFIX)/lib/phonewave-gatekeeper/phonewave-gatekeeper-worker
	install -Dm644 packaging/phonewave-gatekeeper.socket $(DESTDIR)$(PREFIX)/lib/systemd/system/phonewave-gatekeeper.socket
	install -Dm644 packaging/phonewave-gatekeeper@.service $(DESTDIR)$(PREFIX)/lib/systemd/system/phonewave-gatekeeper@.service
	sed -i 's|/usr/lib/phonewave-gatekeeper|$(PREFIX)/lib/phonewave-gatekeeper|g' $(DESTDIR)$(PREFIX)/lib/systemd/system/phonewave-gatekeeper@.service
	install -Dm644 packaging/phonewave-gatekeeper.sysusers $(DESTDIR)$(PREFIX)/lib/sysusers.d/phonewave-gatekeeper.conf
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/phonewave-gatekeeper/LICENSE
