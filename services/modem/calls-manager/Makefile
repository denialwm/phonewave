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
	install -Dm755 $(BIN)/calls-manager $(DESTDIR)$(PREFIX)/bin/calls-manager
	install -Dm644 packaging/calls-manager.service $(DESTDIR)$(PREFIX)/lib/systemd/user/calls-manager.service
	sed -i 's|/usr/bin/calls-manager|$(PREFIX)/bin/calls-manager|g' $(DESTDIR)$(PREFIX)/lib/systemd/user/calls-manager.service
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/calls-manager/LICENSE
