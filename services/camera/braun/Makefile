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
	$(CARGO) clippy --locked --lib --no-default-features -- -D warnings
	$(CARGO) test --locked
install:
	install -Dm755 $(BIN)/braun $(DESTDIR)$(PREFIX)/bin/braun
	install -Dm755 $(BIN)/braunctl $(DESTDIR)$(PREFIX)/bin/braunctl
	install -Dm644 packaging/braun.service $(DESTDIR)$(PREFIX)/lib/systemd/system/braun.service
	install -Dm644 packaging/org.braun.service $(DESTDIR)$(PREFIX)/share/dbus-1/system-services/org.braun.service
	sed -i 's|/usr/bin/braun|$(PREFIX)/bin/braun|g' $(DESTDIR)$(PREFIX)/lib/systemd/system/braun.service $(DESTDIR)$(PREFIX)/share/dbus-1/system-services/org.braun.service
	install -Dm644 packaging/org.braun.conf $(DESTDIR)$(PREFIX)/share/dbus-1/system.d/org.braun.conf
	install -Dm644 packaging/braun.sysusers $(DESTDIR)$(PREFIX)/lib/sysusers.d/braun.conf
	install -d $(DESTDIR)$(PREFIX)/share/braun/sensors
	install -m644 sensors/*.toml $(DESTDIR)$(PREFIX)/share/braun/sensors/
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/braun/LICENSE
