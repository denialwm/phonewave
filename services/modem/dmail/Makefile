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
	install -Dm755 target/release/dmail $(DESTDIR)$(PREFIX)/bin/dmail
	install -Dm755 target/release/dmailctl $(DESTDIR)$(PREFIX)/bin/dmailctl
	install -Dm644 packaging/dmail.service $(DESTDIR)$(PREFIX)/lib/systemd/system/dmail.service
	install -Dm644 packaging/org.dmail.service $(DESTDIR)$(PREFIX)/share/dbus-1/system-services/org.dmail.service
	sed -i 's|/usr/bin/dmail|$(PREFIX)/bin/dmail|g' $(DESTDIR)$(PREFIX)/lib/systemd/system/dmail.service $(DESTDIR)$(PREFIX)/share/dbus-1/system-services/org.dmail.service
	install -Dm644 packaging/org.dmail.conf $(DESTDIR)$(PREFIX)/share/dbus-1/system.d/org.dmail.conf
	install -Dm644 packaging/dmail.sysusers $(DESTDIR)$(PREFIX)/lib/sysusers.d/dmail.conf
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/dmail/LICENSE
