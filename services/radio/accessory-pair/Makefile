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
	install -Dm755 $(BIN)/accessory-pair $(DESTDIR)$(PREFIX)/bin/accessory-pair
	install -Dm644 packaging/accessory-pair.service $(DESTDIR)$(PREFIX)/lib/systemd/system/accessory-pair.service
	sed -i 's|/usr/bin/accessory-pair|$(PREFIX)/bin/accessory-pair|g' $(DESTDIR)$(PREFIX)/lib/systemd/system/accessory-pair.service
	install -Dm644 packaging/70-accessory-pair.rules $(DESTDIR)$(PREFIX)/lib/udev/rules.d/70-accessory-pair.rules
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/accessory-pair/LICENSE
