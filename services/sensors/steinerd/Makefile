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
	install -Dm755 target/release/steinerd $(DESTDIR)$(PREFIX)/bin/steinerd
	install -Dm755 target/release/steinerctl $(DESTDIR)$(PREFIX)/bin/steinerctl
	install -Dm644 packaging/steinerd.service $(DESTDIR)$(PREFIX)/lib/systemd/system/steinerd.service
	install -Dm644 packaging/org.steinerd.service $(DESTDIR)$(PREFIX)/share/dbus-1/system-services/org.steinerd.service
	sed -i 's|/usr/bin/steinerd|$(PREFIX)/bin/steinerd|g' $(DESTDIR)$(PREFIX)/lib/systemd/system/steinerd.service $(DESTDIR)$(PREFIX)/share/dbus-1/system-services/org.steinerd.service
	install -Dm644 packaging/org.steinerd.conf $(DESTDIR)$(PREFIX)/share/dbus-1/system.d/org.steinerd.conf
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/steinerd/LICENSE
