PREFIX ?= /usr
DESTDIR ?=
CARGO ?= cargo

.PHONY: all check install
all:
	$(CARGO) build --release --locked
check:
	$(CARGO) fmt --check
	$(CARGO) clippy --locked --all-targets -- -D warnings
	$(CARGO) test --locked
install:
	install -Dm755 target/release/hapticd $(DESTDIR)$(PREFIX)/bin/hapticd
	install -Dm755 target/release/hapticctl $(DESTDIR)$(PREFIX)/bin/hapticctl
	install -Dm644 packaging/hapticd.service $(DESTDIR)$(PREFIX)/lib/systemd/system/hapticd.service
	install -Dm644 packaging/org.hapticd.service $(DESTDIR)$(PREFIX)/share/dbus-1/system-services/org.hapticd.service
	sed -i 's|/usr/bin/hapticd|$(PREFIX)/bin/hapticd|g' $(DESTDIR)$(PREFIX)/lib/systemd/system/hapticd.service $(DESTDIR)$(PREFIX)/share/dbus-1/system-services/org.hapticd.service
	install -Dm644 packaging/org.hapticd.conf $(DESTDIR)$(PREFIX)/share/dbus-1/system.d/org.hapticd.conf
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/hapticd/LICENSE
