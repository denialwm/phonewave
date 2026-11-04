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
	install -Dm755 $(BIN)/hapticd $(DESTDIR)$(PREFIX)/bin/hapticd
	install -Dm755 $(BIN)/hapticctl $(DESTDIR)$(PREFIX)/bin/hapticctl
	install -Dm644 packaging/hapticd.service $(DESTDIR)$(PREFIX)/lib/systemd/system/hapticd.service
	install -Dm644 packaging/org.hapticd.service $(DESTDIR)$(PREFIX)/share/dbus-1/system-services/org.hapticd.service
	sed -i 's|/usr/bin/hapticd|$(PREFIX)/bin/hapticd|g' $(DESTDIR)$(PREFIX)/lib/systemd/system/hapticd.service $(DESTDIR)$(PREFIX)/share/dbus-1/system-services/org.hapticd.service
	install -Dm644 packaging/org.hapticd.conf $(DESTDIR)$(PREFIX)/share/dbus-1/system.d/org.hapticd.conf
	install -Dm644 packaging/70-hapticd-builtin.rules $(DESTDIR)$(PREFIX)/lib/udev/rules.d/70-hapticd-builtin.rules
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/hapticd/LICENSE
