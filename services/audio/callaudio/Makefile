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
	install -Dm755 $(BIN)/callaudio $(DESTDIR)$(PREFIX)/bin/callaudio
	install -Dm644 packaging/callaudio.service $(DESTDIR)$(PREFIX)/lib/systemd/user/callaudio.service
	install -Dm644 packaging/org.callaudio.service $(DESTDIR)$(PREFIX)/share/dbus-1/services/org.callaudio.service
	sed -i 's|/usr/bin/callaudio|$(PREFIX)/bin/callaudio|g' $(DESTDIR)$(PREFIX)/lib/systemd/user/callaudio.service $(DESTDIR)$(PREFIX)/share/dbus-1/services/org.callaudio.service
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/callaudio/LICENSE
